//! WGPU renderer: atlas texture, opaque + transparent pipelines, fog.

use crate::atlas::{decode_atlas, tile_uv, Tile, TILE_COUNT};
use crate::camera::{Camera, Frustum};
use crate::chunk::{ChunkPos, CHUNK_SY};
use crate::mesh::{ChunkMesh, Vertex};
use crate::world::World;

use std::collections::HashMap;
use std::num::NonZeroU64;
use wgpu::util::DeviceExt;

pub const FOG_START: f32 = 14.0 * 16.0;
pub const FOG_END:   f32 = 16.0 * 16.0;

/// Underwater fog band (blocks): pulled in tight so submerged visibility is short
/// and distant terrain dissolves into the murky water colour.
pub const UNDERWATER_FOG_START: f32 = 0.5;
pub const UNDERWATER_FOG_END:   f32 = 22.0;

/// Fixed size of the uv-rect table shared with the vertex shader (`block.wgsl`
/// declares `array<vec4<f32>, UV_RECTS_LEN>`). Sized with headroom over the tile
/// count so adding a few tiles needs no shader edit.
pub const UV_RECTS_LEN: usize = 16;

pub struct Renderer {
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    pub atlas_texture: wgpu::Texture,
    pub atlas_view: wgpu::TextureView,
    pub atlas_sampler: wgpu::Sampler,
    pub opaque_pipe: wgpu::RenderPipeline,
    pub transparent_pipe: wgpu::RenderPipeline,
    /// Pipeline for the targeted-block wireframe (LineList, black, view_proj only).
    pub outline_pipe: wgpu::RenderPipeline,
    pub outline_bind: wgpu::BindGroup,
    /// 24 line vertices (12 cube edges) for the selection box; rewritten only
    /// when the selected block changes (see `selection` / `selection_drawn`).
    pub outline_vbuf: wgpu::Buffer,
    /// Currently-targeted block (min corner), or None when nothing is targeted.
    pub selection: Option<glam::IVec3>,
    /// The block whose geometry currently sits in `outline_vbuf`.
    selection_drawn: Option<glam::IVec3>,
    pub uniform_buf: wgpu::Buffer,
    pub uniform_bind: wgpu::BindGroup,
    pub atlas_bind: wgpu::BindGroup,
    pub depth: wgpu::TextureView,
    pub chunk_meshes: HashMap<ChunkPos, GpuMesh>,
    /// Camera frustum for viewspace culling, refreshed each frame in
    /// `update_uniforms`; chunk meshes outside it are skipped in `render`.
    pub frustum: Frustum,
    /// Camera world position, refreshed in `update_uniforms`; used to sort
    /// chunk draws front-to-back (opaque) / back-to-front (transparent).
    pub cam_pos: glam::Vec3,
    /// Background clear colour, kept in sync with the fog colour each frame (sky/
    /// biome fog above water, deep blue when submerged) so the horizon matches the
    /// fog the terrain fades into.
    pub clear_color: [f32; 3],
}

pub struct GpuMesh {
    pub opaque_vbuf: Option<wgpu::Buffer>,
    pub opaque_ibuf: Option<wgpu::Buffer>,
    pub opaque_idx_count: u32,
    pub transparent_vbuf: Option<wgpu::Buffer>,
    pub transparent_ibuf: Option<wgpu::Buffer>,
    pub transparent_idx_count: u32,
    pub origin: (i32, i32),
}

#[repr(C, align(16))]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Uniforms {
    pub view_proj: [[f32; 4]; 4],
    pub cam_pos: [f32; 4],  // padded to 16
    pub fog: [f32; 4],      // (start, end, _, _)
    pub fog_color: [f32; 4],
}

pub async fn new_renderer_from_target(
    target: impl Into<wgpu::SurfaceTarget<'static>>,
    width: u32,
    height: u32,
) -> Renderer {
    let instance = wgpu::Instance::new(&instance_descriptor());
    let surface = instance.create_surface(target).expect("create surface");
    new_renderer_inner(instance, surface, width, height).await
}

pub async fn new_renderer_with_instance(
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    width: u32,
    height: u32,
) -> Renderer {
    new_renderer_inner(instance, surface, width, height).await
}

/// Instance descriptor that picks appropriate backends per platform:
/// native = all (Vulkan/Metal/DX12/GL); web = WebGPU with WebGL fallback.
pub fn instance_descriptor() -> wgpu::InstanceDescriptor {
    #[cfg(target_arch = "wasm32")]
    {
        wgpu::InstanceDescriptor {
            backends: wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL,
            flags: wgpu::InstanceFlags::default(),
            ..Default::default()
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        wgpu::InstanceDescriptor::default()
    }
}

pub async fn new_renderer(
    surface: wgpu::Surface<'static>,
    width: u32,
    height: u32,
) -> Renderer {
    // NOTE: surface must be created from a wgpu::Instance that is *not*
    // dropped before this call. We create a fresh instance here which means
    // the caller must have created the surface from this same runtime. In
    // practice, prefer `new_renderer_from_target` so the surface and adapter
    // share the same instance.
    let instance = wgpu::Instance::new(&instance_descriptor());
    new_renderer_inner(instance, surface, width, height).await
}

async fn new_renderer_inner(
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    width: u32,
    height: u32,
) -> Renderer {
    // Try a high-performance adapter first; on browsers without WebGPU this
    // may still succeed via the WebGL fallback. If it fails entirely (no
    // adapter compatible with the surface), retry with force_fallback_adapter
    // to accept the software/lowest-tier adapter rather than panicking.
    let adapter = match instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: Some(&surface),
        force_fallback_adapter: false,
    }).await {
        Ok(a) => a,
        Err(_) => {
            #[cfg(target_arch = "wasm32")]
            web_sys::console::warn_1(
                &"wgpu: primary adapter unavailable; trying fallback".into(),
            );
            #[cfg(not(target_arch = "wasm32"))]
            eprintln!("wgpu: primary adapter unavailable; trying fallback");
            instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: Some(&surface),
                force_fallback_adapter: true,
            }).await
                .expect("no wgpu adapter available (WebGPU/WebGL both failed)")
        }
    };
    let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor {
        label: None,
        required_features: wgpu::Features::empty(),
        required_limits: {
            #[cfg(target_arch = "wasm32")]
            { wgpu::Limits::downlevel_webgl2_defaults().using_alignment(adapter.limits()) }
            #[cfg(not(target_arch = "wasm32"))]
            { wgpu::Limits::default().using_alignment(adapter.limits()) }
        },
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }).await.expect("device");

    let config = surface.get_default_config(&adapter, width, height)
        .expect("surface config");
    let format = config.format;
    let sample_count = 1u32;
    surface.configure(&device, &config);

    let (atlas_texture, atlas_view, atlas_sampler) = create_atlas(&device, &queue);
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("block shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shaders/block.wgsl").into()),
    });

    let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("uniforms"),
        contents: bytemuck::cast_slice(&[Uniforms {
            view_proj: glam::Mat4::IDENTITY.to_cols_array_2d(),
            cam_pos: [0.0; 4],
            fog: [FOG_START, FOG_END, 0.0, 0.0],
            fog_color: [0.62, 0.78, 0.95, 1.0],
        }]),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });

    // uv-rect table: the EXACT `tile_uv()` bits per tile, indexed by `Tile as
    // usize`. The vertex shader only SELECTS corners from this (no arithmetic),
    // so reconstructed uvs are bit-identical to the old CPU-baked per-vertex uvs
    // on every backend (incl. WebGL2). Never updated after creation.
    const _: () = assert!(TILE_COUNT <= UV_RECTS_LEN);
    let mut uv_rects = [[0f32; 4]; UV_RECTS_LEN];
    for &t in Tile::ALL {
        uv_rects[t as usize] = tile_uv(t);
    }
    let uv_rects_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("uv_rects"),
        contents: bytemuck::cast_slice(&uv_rects[..]),
        usage: wgpu::BufferUsages::UNIFORM,
    });

    let uniform_bind_layout = wgpu::BindGroupLayoutDescriptor {
        label: Some("uniform bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0, visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(std::mem::size_of::<Uniforms>() as u64),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1, visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new((UV_RECTS_LEN * 16) as u64),
                },
                count: None,
            },
        ],
    };
    let uniform_bgl = device.create_bind_group_layout(&uniform_bind_layout);
    let uniform_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("uniform bg"),
        layout: &uniform_bgl,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: uniform_buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: uv_rects_buf.as_entire_binding() },
        ],
    });

    let atlas_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("atlas bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1, visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let atlas_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("atlas bg"),
        layout: &atlas_bgl,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&atlas_view) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&atlas_sampler) },
        ],
    });

    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("pipe layout"),
        bind_group_layouts: &[&uniform_bgl, &atlas_bgl],
        push_constant_ranges: &[],
    });

    // 28-byte packed vertex: pos (f32x3) + tint (f32x3) + packed (u32).
    let vbuf_attrs = [
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x3, offset: 12, shader_location: 1,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Uint32, offset: 24, shader_location: 2,
        },
    ];
    let vbuf_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<Vertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &vbuf_attrs,
    };

    let opaque_targets = vec![Some(wgpu::ColorTargetState {
        format, blend: Some(wgpu::BlendState::REPLACE),
        write_mask: wgpu::ColorWrites::ALL,
    })];
    let transparent_targets = vec![Some(wgpu::ColorTargetState {
        format, blend: Some(wgpu::BlendState::ALPHA_BLENDING),
        write_mask: wgpu::ColorWrites::ALL,
    })];

    let opaque_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("opaque pipe"),
        layout: Some(&layout),
        vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_main"), compilation_options: Default::default(), buffers: &[vbuf_layout.clone()] },
        fragment: Some(wgpu::FragmentState {
            module: &shader, entry_point: Some("fs_opaque"), compilation_options: Default::default(), targets: &opaque_targets,
        }),
        primitive: wgpu::PrimitiveState { cull_mode: Some(wgpu::Face::Back), ..Default::default() },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState { count: sample_count, ..Default::default() },
        multiview: None,
        cache: None,
    });
    let transparent_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("transparent pipe"),
        layout: Some(&layout),
        vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_main"), compilation_options: Default::default(), buffers: &[vbuf_layout] },
        fragment: Some(wgpu::FragmentState {
            module: &shader, entry_point: Some("fs_transparent"), compilation_options: Default::default(), targets: &transparent_targets,
        }),
        // Double-sided: water faces must be visible from underneath (looking up at
        // the surface while submerged) as well as from above.
        primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState { count: sample_count, ..Default::default() },
        multiview: None,
        cache: None,
    });

    // --- Selection-outline pipeline. ---
    // Its own minimal bind-group layout (Uniforms at binding 0 only) so it
    // doesn't couple to the block pipelines' uv_rects layout. Reuses the same
    // uniform buffer for view_proj.
    let outline_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("outline shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shaders/outline.wgsl").into()),
    });
    let outline_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("outline bgl"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: NonZeroU64::new(std::mem::size_of::<Uniforms>() as u64),
            },
            count: None,
        }],
    });
    let outline_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("outline bg"),
        layout: &outline_bgl,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform_buf.as_entire_binding() }],
    });
    let outline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("outline layout"),
        bind_group_layouts: &[&outline_bgl],
        push_constant_ranges: &[],
    });
    let outline_vbuf_layout = wgpu::VertexBufferLayout {
        array_stride: 12, // vec3<f32>
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0,
        }],
    };
    let outline_targets = [Some(wgpu::ColorTargetState {
        format, blend: Some(wgpu::BlendState::REPLACE), write_mask: wgpu::ColorWrites::ALL,
    })];
    let outline_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("outline pipe"),
        layout: Some(&outline_layout),
        vertex: wgpu::VertexState {
            module: &outline_shader, entry_point: Some("vs_outline"),
            compilation_options: Default::default(), buffers: &[outline_vbuf_layout],
        },
        fragment: Some(wgpu::FragmentState {
            module: &outline_shader, entry_point: Some("fs_outline"),
            compilation_options: Default::default(), targets: &outline_targets,
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::LineList,
            ..Default::default()
        },
        // Depth-test against terrain so edges behind blocks are hidden, but
        // don't write depth. The box is inflated slightly outward (see
        // `outline_vertices`) so visible front edges win the LessEqual test.
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::LessEqual,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState { count: sample_count, ..Default::default() },
        multiview: None,
        cache: None,
    });
    // 24 vertices × vec3<f32> = 288 bytes (12 edges).
    let outline_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("outline vbuf"),
        size: 24 * 12,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let depth = create_depth(&device, width, height);

    Renderer {
        surface, device, queue, config,
        atlas_texture, atlas_view, atlas_sampler,
        opaque_pipe, transparent_pipe,
        outline_pipe, outline_bind, outline_vbuf,
        selection: None, selection_drawn: None,
        uniform_buf, uniform_bind, atlas_bind,
        depth,
        chunk_meshes: HashMap::new(),
        frustum: Frustum::permissive(),
        cam_pos: glam::Vec3::ZERO,
        clear_color: [0.62, 0.78, 0.95],
    }
}

/// The 24 line-segment endpoints (12 edges) of the wireframe cube for block
/// `b`, in world space. Inflated outward by `INFLATE` so visible front edges
/// sit a hair nearer the camera than the block surface and pass the LessEqual
/// depth test (no z-fighting); back edges remain occluded by the block itself.
fn outline_vertices(b: glam::IVec3) -> [[f32; 3]; 24] {
    const INFLATE: f32 = 0.003;
    let lo = [b.x as f32 - INFLATE, b.y as f32 - INFLATE, b.z as f32 - INFLATE];
    let hi = [b.x as f32 + 1.0 + INFLATE, b.y as f32 + 1.0 + INFLATE, b.z as f32 + 1.0 + INFLATE];
    // 8 corners indexed by (x_hi?, y_hi?, z_hi?).
    let c = |xh: bool, yh: bool, zh: bool| {
        [if xh { hi[0] } else { lo[0] },
         if yh { hi[1] } else { lo[1] },
         if zh { hi[2] } else { lo[2] }]
    };
    let c000 = c(false, false, false);
    let c100 = c(true, false, false);
    let c010 = c(false, true, false);
    let c001 = c(false, false, true);
    let c110 = c(true, true, false);
    let c101 = c(true, false, true);
    let c011 = c(false, true, true);
    let c111 = c(true, true, true);
    [
        // bottom rectangle (y = lo)
        c000, c100, c100, c101, c101, c001, c001, c000,
        // top rectangle (y = hi)
        c010, c110, c110, c111, c111, c011, c011, c010,
        // four vertical edges
        c000, c010, c100, c110, c101, c111, c001, c011,
    ]
}

fn create_atlas(device: &wgpu::Device, queue: &wgpu::Queue)
    -> (wgpu::Texture, wgpu::TextureView, wgpu::Sampler)
{
    let (rgba, w, h) = decode_atlas();
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("atlas"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w * 4),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("atlas sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });
    (texture, view, sampler)
}

fn create_depth(device: &wgpu::Device, w: u32, h: u32) -> wgpu::TextureView {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}

impl Renderer {
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 { return; }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.depth = create_depth(&self.device, width, height);
    }

    pub fn update_uniforms(&mut self, cam: &Camera, fog_color: [f32; 3], time: f32, underwater: bool) {
        let view_proj = cam.view_proj();
        // Refresh the culling frustum from the same matrix the GPU will use.
        self.frustum = Frustum::from_view_proj(view_proj);
        self.cam_pos = cam.pos;
        self.clear_color = fog_color;
        let (fog_start, fog_end) = if underwater {
            (UNDERWATER_FOG_START, UNDERWATER_FOG_END)
        } else {
            (FOG_START, FOG_END)
        };
        let u = Uniforms {
            view_proj: view_proj.to_cols_array_2d(),
            cam_pos: [cam.pos.x, cam.pos.y, cam.pos.z, 0.0],
            // fog.z = animation time (caustics), fog.w = underwater flag.
            fog: [fog_start, fog_end, time, if underwater { 1.0 } else { 0.0 }],
            fog_color: [fog_color[0], fog_color[1], fog_color[2], 1.0],
        };
        self.queue.write_buffer(&self.uniform_buf, 0, bytemuck::cast_slice(&[u]));
    }

    /// Set (or clear) the block highlighted by the selection outline. Cheap: the
    /// vertex buffer is only re-uploaded in `render` when the target changes.
    pub fn set_selection(&mut self, block: Option<glam::IVec3>) {
        self.selection = block;
    }

    /// Is this chunk mesh's bounding box inside the current view frustum?
    #[inline]
    fn chunk_visible(&self, gm: &GpuMesh) -> bool {
        let (ox, oz) = gm.origin;
        let min = glam::Vec3::new(ox as f32, 0.0, oz as f32);
        let max = glam::Vec3::new((ox + 16) as f32, CHUNK_SY as f32, (oz + 16) as f32);
        self.frustum.aabb_visible(min, max)
    }

    /// Synchronize GPU meshes with the World's CPU meshes.
    pub fn sync_meshes(&mut self, world: &mut World) {
        // Upload only meshes marked dirty by the world (newly built/changed).
        // Existing unchanged meshes are left on the GPU untouched.
        let mut keep: std::collections::HashSet<ChunkPos> = std::collections::HashSet::new();
        for (pos, mesh) in world.iter_meshes() {
            keep.insert(pos);
            let need_upload = match self.chunk_meshes.get(&pos) {
                None => true,
                Some(_) => mesh.mesh_dirty,
            };
            if need_upload {
                let gm = upload_mesh(&self.device, mesh, pos);
                self.chunk_meshes.insert(pos, gm);
            }
        }
        // Drop removed.
        self.chunk_meshes.retain(|p, _| keep.contains(p));
        // Clear CPU-side dirty flags now that uploads are done.
        for pos in keep {
            if let Some(m) = world.meshes.get_mut(&pos) {
                m.mesh_dirty = false;
            }
        }
    }

    pub fn render(&mut self) {
        let frame = match self.surface.get_current_texture() {
            Ok(t) => t,
            Err(_) => return,
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Refresh the outline vertex buffer only when the target changed.
        if self.selection != self.selection_drawn {
            if let Some(b) = self.selection {
                let verts = outline_vertices(b);
                self.queue.write_buffer(&self.outline_vbuf, 0, bytemuck::cast_slice(&verts));
            }
            self.selection_drawn = self.selection;
        }

        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frame"),
        });
        // Frustum-cull + depth-sort the visible chunks once. The opaque pass
        // draws nearest-first so the GPU's early-Z rejects occluded fragments
        // before the (texture + tint + fog) fragment shader runs — cutting
        // overdraw, which is the dominant GPU cost in dense voxel terrain. The
        // transparent pass draws farthest-first for correct back-to-front alpha.
        let cam = self.cam_pos;
        let mut order: Vec<(f32, &GpuMesh)> = self.chunk_meshes.values()
            .filter(|gm| self.chunk_visible(gm))
            .map(|gm| {
                let (ox, oz) = gm.origin;
                let c = glam::Vec3::new(ox as f32 + 8.0, CHUNK_SY as f32 * 0.5, oz as f32 + 8.0);
                ((cam - c).length_squared(), gm)
            })
            .collect();
        order.sort_by(|a, b| a.0.total_cmp(&b.0));
        let cc = self.clear_color;
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("opaque pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: cc[0] as f64, g: cc[1] as f64, b: cc[2] as f64, a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            pass.set_bind_group(1, &self.atlas_bind, &[]);
            pass.set_pipeline(&self.opaque_pipe);
            for (_, gm) in order.iter() { // near -> far (early-Z)
                if let (Some(vb), Some(ib)) = (&gm.opaque_vbuf, &gm.opaque_ibuf) {
                    if gm.opaque_idx_count == 0 { continue; }
                    pass.set_vertex_buffer(0, vb.slice(..));
                    pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..gm.opaque_idx_count, 0, 0..1);
                }
            }
        }
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("transparent pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            pass.set_bind_group(1, &self.atlas_bind, &[]);
            pass.set_pipeline(&self.transparent_pipe);
            for (_, gm) in order.iter().rev() { // far -> near (alpha order)
                if let (Some(vb), Some(ib)) = (&gm.transparent_vbuf, &gm.transparent_ibuf) {
                    if gm.transparent_idx_count == 0 { continue; }
                    pass.set_vertex_buffer(0, vb.slice(..));
                    pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..gm.transparent_idx_count, 0, 0..1);
                }
            }
        }
        // Selection outline, last: load color + depth, depth-test (no write) so
        // it draws over terrain/water at the targeted block but stays occluded
        // behind nearer geometry.
        if self.selection.is_some() {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("outline pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.outline_pipe);
            pass.set_bind_group(0, &self.outline_bind, &[]);
            pass.set_vertex_buffer(0, self.outline_vbuf.slice(..));
            pass.draw(0..24, 0..1);
        }
        self.queue.submit(std::iter::once(enc.finish()));
        frame.present();
    }
}

fn upload_mesh(device: &wgpu::Device, mesh: &ChunkMesh, pos: ChunkPos) -> GpuMesh {
    use wgpu::util::DeviceExt;
    let opaque_vbuf = if mesh.opaque.is_empty() { None } else {
        Some(device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&mesh.opaque),
            usage: wgpu::BufferUsages::VERTEX,
        }))
    };
    let opaque_ibuf = if mesh.opaque_idx.is_empty() { None } else {
        Some(device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&mesh.opaque_idx),
            usage: wgpu::BufferUsages::INDEX,
        }))
    };
    let transparent_vbuf = if mesh.transparent.is_empty() { None } else {
        Some(device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&mesh.transparent),
            usage: wgpu::BufferUsages::VERTEX,
        }))
    };
    let transparent_ibuf = if mesh.transparent_idx.is_empty() { None } else {
        Some(device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&mesh.transparent_idx),
            usage: wgpu::BufferUsages::INDEX,
        }))
    };
    GpuMesh {
        opaque_vbuf, opaque_ibuf,
        opaque_idx_count: mesh.opaque_idx.len() as u32,
        transparent_vbuf, transparent_ibuf,
        transparent_idx_count: mesh.transparent_idx.len() as u32,
        origin: (pos.cx * 16, pos.cz * 16),
    }
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod gpu_validation {
    use super::*;

    /// Headless validation that the packed-vertex pipeline is internally
    /// consistent: WGSL parses + passes naga validation, the vertex attribute
    /// formats/locations match the shader's `VsIn`, and the bind-group layouts
    /// match the shader's declared bindings (group0: Uniforms + uv_rects;
    /// group1: atlas texture + sampler). Any mismatch surfaces as a captured
    /// validation error. Skips cleanly on machines/CI with no GPU adapter
    /// (the interactive demo is where final visual confirmation happens).
    #[test]
    fn packed_vertex_pipeline_validates() {
        let instance = wgpu::Instance::new(&instance_descriptor());
        let adapter = match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
        })) {
            Ok(a) => a,
            Err(_) => { eprintln!("[skip] no wgpu adapter; pipeline validation not run"); return; }
        };
        let (device, _queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: None,
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default().using_alignment(adapter.limits()),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        })).expect("device");

        device.push_error_scope(wgpu::ErrorFilter::Validation);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("block shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/block.wgsl").into()),
        });

        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0, visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: NonZeroU64::new(std::mem::size_of::<Uniforms>() as u64) },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1, visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: NonZeroU64::new((UV_RECTS_LEN * 16) as u64) },
                    count: None,
                },
            ],
        });
        let atlas_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None, bind_group_layouts: &[&uniform_bgl, &atlas_bgl], push_constant_ranges: &[],
        });

        let vbuf_attrs = [
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 12, shader_location: 1 },
            wgpu::VertexAttribute { format: wgpu::VertexFormat::Uint32, offset: 24, shader_location: 2 },
        ];
        let vbuf_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &vbuf_attrs,
        };
        let targets = [Some(wgpu::ColorTargetState {
            format: wgpu::TextureFormat::Rgba8UnormSrgb, blend: Some(wgpu::BlendState::REPLACE), write_mask: wgpu::ColorWrites::ALL,
        })];
        let _pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None, layout: Some(&layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_main"), compilation_options: Default::default(), buffers: &[vbuf_layout] },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs_opaque"), compilation_options: Default::default(), targets: &targets }),
            primitive: wgpu::PrimitiveState { cull_mode: Some(wgpu::Face::Back), ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState { format: wgpu::TextureFormat::Depth32Float, depth_write_enabled: true, depth_compare: wgpu::CompareFunction::Less, stencil: wgpu::StencilState::default(), bias: wgpu::DepthBiasState::default() }),
            multisample: wgpu::MultisampleState::default(), multiview: None, cache: None,
        });

        // Also validate the outline pipeline + shader (LineList, group0 = a
        // minimal Uniforms-only bind group).
        let outline_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("outline shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/outline.wgsl").into()),
        });
        let outline_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0, visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: NonZeroU64::new(std::mem::size_of::<Uniforms>() as u64) },
                count: None,
            }],
        });
        let outline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None, bind_group_layouts: &[&outline_bgl], push_constant_ranges: &[],
        });
        let outline_vbuf_layout = wgpu::VertexBufferLayout {
            array_stride: 12, step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 }],
        };
        let _outline_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None, layout: Some(&outline_layout),
            vertex: wgpu::VertexState { module: &outline_shader, entry_point: Some("vs_outline"), compilation_options: Default::default(), buffers: &[outline_vbuf_layout] },
            fragment: Some(wgpu::FragmentState { module: &outline_shader, entry_point: Some("fs_outline"), compilation_options: Default::default(), targets: &targets }),
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::LineList, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState { format: wgpu::TextureFormat::Depth32Float, depth_write_enabled: false, depth_compare: wgpu::CompareFunction::LessEqual, stencil: wgpu::StencilState::default(), bias: wgpu::DepthBiasState::default() }),
            multisample: wgpu::MultisampleState::default(), multiview: None, cache: None,
        });

        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "packed-vertex pipeline validation error: {err:?}");
        // Confirm the assumption baked into the packing: tile ids fit in 8 bits.
        assert!(TILE_COUNT <= 256);
        // Stride sanity: the compressed vertex is exactly 28 bytes.
        assert_eq!(std::mem::size_of::<Vertex>(), 28);
    }
}
