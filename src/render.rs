//! WGPU renderer: atlas texture, opaque + transparent pipelines, fog.

use crate::atlas::decode_atlas;
use crate::camera::{Camera, Frustum};
use crate::chunk::{ChunkPos, CHUNK_SY};
use crate::mesh::{ChunkMesh, Vertex};
use crate::world::World;

use std::collections::HashMap;
use std::num::NonZeroU64;
use wgpu::util::DeviceExt;

pub const FOG_START: f32 = 14.0 * 16.0;
pub const FOG_END:   f32 = 16.0 * 16.0;

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
    pub uniform_buf: wgpu::Buffer,
    pub uniform_bind: wgpu::BindGroup,
    pub atlas_bind: wgpu::BindGroup,
    pub depth: wgpu::TextureView,
    pub chunk_meshes: HashMap<ChunkPos, GpuMesh>,
    /// Camera frustum for viewspace culling, refreshed each frame in
    /// `update_uniforms`; chunk meshes outside it are skipped in `render`.
    pub frustum: Frustum,
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
        ],
    };
    let uniform_bgl = device.create_bind_group_layout(&uniform_bind_layout);
    let uniform_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("uniform bg"),
        layout: &uniform_bgl,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform_buf.as_entire_binding() }],
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

    let vbuf_attrs = [
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2, offset: 12, shader_location: 1,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32, offset: 20, shader_location: 2,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x3, offset: 24, shader_location: 3,
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
        primitive: wgpu::PrimitiveState { cull_mode: Some(wgpu::Face::Back), ..Default::default() },
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

    let depth = create_depth(&device, width, height);

    Renderer {
        surface, device, queue, config,
        atlas_texture, atlas_view, atlas_sampler,
        opaque_pipe, transparent_pipe,
        uniform_buf, uniform_bind, atlas_bind,
        depth,
        chunk_meshes: HashMap::new(),
        frustum: Frustum::permissive(),
    }
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

    pub fn update_uniforms(&mut self, cam: &Camera, fog_color: [f32; 3]) {
        let view_proj = cam.view_proj();
        // Refresh the culling frustum from the same matrix the GPU will use.
        self.frustum = Frustum::from_view_proj(view_proj);
        let u = Uniforms {
            view_proj: view_proj.to_cols_array_2d(),
            cam_pos: [cam.pos.x, cam.pos.y, cam.pos.z, 0.0],
            fog: [FOG_START, FOG_END, 0.0, 0.0],
            fog_color: [fog_color[0], fog_color[1], fog_color[2], 1.0],
        };
        self.queue.write_buffer(&self.uniform_buf, 0, bytemuck::cast_slice(&[u]));
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
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frame"),
        });
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("opaque pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.62, g: 0.78, b: 0.95, a: 1.0,
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
            for (pos, gm) in self.chunk_meshes.iter() {
                let _ = pos;
                if !self.chunk_visible(gm) { continue; } // viewspace (frustum) cull
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
            for (pos, gm) in self.chunk_meshes.iter() {
                let _ = pos;
                if !self.chunk_visible(gm) { continue; } // viewspace (frustum) cull
                if let (Some(vb), Some(ib)) = (&gm.transparent_vbuf, &gm.transparent_ibuf) {
                    if gm.transparent_idx_count == 0 { continue; }
                    pass.set_vertex_buffer(0, vb.slice(..));
                    pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..gm.transparent_idx_count, 0, 0..1);
                }
            }
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
