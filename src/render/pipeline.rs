use crate::atlas::{tile_uv, Tile, TILE_COUNT};
use crate::mesh::Vertex;

use std::num::NonZeroU64;
use wgpu::util::DeviceExt;

use super::crosshair::MAX_CROSSHAIR_VERTICES;
use super::selection::MAX_OUTLINE_VERTICES;
use super::uniforms::{Uniforms, UV_RECTS_LEN};

/// Size of one MVP slot in the model3d dynamic-offset uniform buffer. A `mat4`
/// is 64 bytes but dynamic offsets must be a multiple of the device's
/// `min_uniform_buffer_offset_alignment` (256 on WebGL2 / the WebGPU minimum),
/// so each per-draw MVP occupies a 256-byte aligned slot.
pub(super) const MODEL3D_MVP_SLOT_SIZE: u64 = 256;
/// Number of 256-byte MVP slots in the model3d uniform buffer. The hand uses
/// slot 0; the isometric inventory icons (Layer 4 UI) cycle through the rest, so
/// 64 slots covers the open inventory's 36 visible cube icons with headroom.
pub(super) const MODEL3D_MVP_SLOTS: u64 = 64;
/// Max vertices in the reusable model3d dynamic vertex buffer. A textured cube is
/// 24 verts; this covers the hand plus a batch of icon cubes drawn in one buffer.
pub(super) const MAX_MODEL3D_VERTICES: u64 = 4096;
/// Max indices in the reusable model3d dynamic index buffer (36 per cube).
pub(super) const MAX_MODEL3D_INDICES: u64 = 8192;
/// Max vertices in the reusable item3d dynamic vertex buffer (the extruded held
/// item). A 16×16 sprite extrudes to front+back + boundary walls; a dense flower
/// silhouette is well under this (non-indexed triangle list, 6 verts/quad).
pub(super) const MAX_ITEM3D_VERTICES: u64 = 4096;
/// Vertices in the break-overlay dynamic vbuf: exactly one inflated cube (24).
pub(super) const MAX_BREAK_VERTICES: u64 = 24;
/// Indices in the break-overlay dynamic ibuf: one cube (36).
pub(super) const MAX_BREAK_INDICES: u64 = 36;
/// Max vertices in the item-entity dynamic vbuf. A stack draws up to 5 layered
/// copies (120 verts per cube / 40 per sprite), so this is sized 5× the old
/// single-copy budget to still cover ~170 simultaneously-visible dropped items
/// (more when they're single, unstacked drops) without the bake overflowing and
/// dropping every item entity that frame.
pub(super) const MAX_ITEM_ENTITY_VERTICES: u64 = 20480;
/// Max indices in the item-entity dynamic ibuf (up to 180 per cube / 30 per
/// sprite for a 5-layer stack), matching [`MAX_ITEM_ENTITY_VERTICES`].
pub(super) const MAX_ITEM_ENTITY_INDICES: u64 = 30720;
/// Max vertices in the reusable UI dynamic vbuf (gui quads + digit cells). The
/// open inventory is ~40 slots + a 176×166 panel; digits are a few quads each.
/// 6 verts/quad; 16384 covers the full open inventory with comfortable headroom.
pub(super) const MAX_UI_VERTICES: u64 = 16384;

pub(super) struct PipelineResources {
    pub uniform_bind: wgpu::BindGroup,
    pub atlas_bind: wgpu::BindGroup,
    pub sky_pipe: wgpu::RenderPipeline,
    pub sky_bind: wgpu::BindGroup,
    pub opaque_pipe: wgpu::RenderPipeline,
    pub transparent_pipe: wgpu::RenderPipeline,
    pub outline_pipe: wgpu::RenderPipeline,
    pub outline_bind: wgpu::BindGroup,
    pub outline_vbuf: wgpu::Buffer,
    pub crosshair_pipe: wgpu::RenderPipeline,
    pub crosshair_vbuf: wgpu::Buffer,
    /// model3d pipeline: per-draw MVP (dynamic offset) + block atlas, full-bright,
    /// NO depth. Serves the isometric slot icons in the depthless UI pass.
    pub model3d_pipe: wgpu::RenderPipeline,
    /// Same shader/layout as `model3d_pipe` but WITH a depth attachment (Depth32
    /// Float, write, Less). Used for the first-person held block in the hand pass,
    /// which now carries a cleared depth buffer so the held geometry self-sorts.
    pub model3d_hand_pipe: wgpu::RenderPipeline,
    /// Dynamic-offset uniform buffer holding up to [`MODEL3D_MVP_SLOTS`] MVP
    /// matrices in 256-byte slots; written per frame by the hand / icon passes.
    pub model3d_mvp_buf: wgpu::Buffer,
    /// group(0) bind for model3d: the MVP buffer (dynamic offset) + the shared
    /// uv_rects table at binding 1. Bound with the per-draw 256-aligned offset.
    pub model3d_mvp_bind: wgpu::BindGroup,
    /// Reusable dynamic vertex buffer for model3d draws (hand + icons).
    pub model3d_vbuf: wgpu::Buffer,
    /// Reusable dynamic index buffer for model3d draws.
    pub model3d_ibuf: wgpu::Buffer,
    /// item3d pipeline: the EXTRUDED first-person held item (flowers / tools).
    /// Explicit per-vertex (pos, uv, shade); group(0) = a dynamic-offset MVP over
    /// the shared `model3d_mvp_buf`; group(1) = the block atlas. Full-bright,
    /// alpha-cutout, double-sided, depth test + write (the hand pass clears depth)
    /// so the front/back/side-wall faces self-sort instead of overdrawing.
    pub item3d_pipe: wgpu::RenderPipeline,
    /// group(0) bind for item3d: just the dynamic-offset MVP (binding 0) over the
    /// shared `model3d_mvp_buf` — reuses slot 0 (the hand slot is free for a held
    /// sprite, which emits no model3d geometry).
    pub item3d_mvp_bind: wgpu::BindGroup,
    /// Reusable dynamic vbuf for the extruded held-item geometry (non-indexed
    /// triangle list, rewritten in place per frame).
    pub item3d_vbuf: wgpu::Buffer,
    /// Break-overlay pipeline: the cracked-block destroy quad. Reuses the block
    /// `uniform_bind` (view_proj + uv_rects) + `atlas_bind`, alpha-blended, depth
    /// LessEqual / no-write, geometry slightly inflated.
    pub break_pipe: wgpu::RenderPipeline,
    /// Reusable dynamic vbuf for the break overlay (one inflated cube).
    pub break_vbuf: wgpu::Buffer,
    /// Reusable dynamic ibuf for the break overlay (one cube).
    pub break_ibuf: wgpu::Buffer,
    /// Reusable dynamic vbuf for item-entity geometry (drawn by the opaque pipe).
    pub item_entity_vbuf: wgpu::Buffer,
    /// Reusable dynamic ibuf for item-entity geometry.
    pub item_entity_ibuf: wgpu::Buffer,
    /// Particle pipeline: camera-facing billboards. Reuses the block `uniform_bind`
    /// + `atlas_bind`, alpha-blended, depth-test (Load) / no-write.
    pub particle_pipe: wgpu::RenderPipeline,
    /// Reusable dynamic vbuf for particle quads (rewritten in place per frame).
    pub particle_vbuf: wgpu::Buffer,
    /// Static ibuf for particle quads (6 per quad), uploaded once.
    pub particle_ibuf: wgpu::Buffer,
    /// UI pipeline: 2D HUD / inventory quads (NDC pos + uv + color). Samples the
    /// SEPARATE gui atlas (`ui_bind`), alpha-blended, NO depth, drawn last.
    pub ui_pipe: wgpu::RenderPipeline,
    /// group(0) bind for the UI pass: the gui sprite atlas (texture + sampler).
    pub ui_bind: wgpu::BindGroup,
    /// Reusable dynamic vbuf for UI quads (rewritten in place per frame).
    pub ui_vbuf: wgpu::Buffer,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn create_pipeline_resources(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    sample_count: u32,
    uniform_buf: &wgpu::Buffer,
    atlas_view: &wgpu::TextureView,
    atlas_sampler: &wgpu::Sampler,
    gui_view: &wgpu::TextureView,
    gui_sampler: &wgpu::Sampler,
) -> PipelineResources {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("block shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/block.wgsl").into()),
    });
    let sky_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("sky shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/sky.wgsl").into()),
    });
    let crosshair_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("crosshair shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/crosshair.wgsl").into()),
    });

    // uv-rect table: the EXACT `tile_uv()` bits per tile, indexed by `Tile as
    // usize`. The vertex shader only SELECTS corners from this (no arithmetic),
    // so reconstructed uvs are bit-identical to the old CPU-baked per-vertex uvs
    // on every backend. Never updated after creation.
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
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(std::mem::size_of::<Uniforms>() as u64),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::VERTEX,
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
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: uv_rects_buf.as_entire_binding(),
            },
        ],
    });

    let atlas_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("atlas bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let atlas_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("atlas bg"),
        layout: &atlas_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(atlas_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(atlas_sampler),
            },
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
            format: wgpu::VertexFormat::Float32x3,
            offset: 0,
            shader_location: 0,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x3,
            offset: 12,
            shader_location: 1,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Uint32,
            offset: 24,
            shader_location: 2,
        },
    ];
    let vbuf_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<Vertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &vbuf_attrs,
    };

    let opaque_targets = vec![Some(wgpu::ColorTargetState {
        format,
        blend: Some(wgpu::BlendState::REPLACE),
        write_mask: wgpu::ColorWrites::ALL,
    })];
    let transparent_targets = vec![Some(wgpu::ColorTargetState {
        format,
        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
        write_mask: wgpu::ColorWrites::ALL,
    })];

    let opaque_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("opaque pipe"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: std::slice::from_ref(&vbuf_layout),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_opaque"),
            compilation_options: Default::default(),
            targets: &opaque_targets,
        }),
        primitive: wgpu::PrimitiveState {
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: sample_count,
            ..Default::default()
        },
        multiview: None,
        cache: None,
    });
    let transparent_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("transparent pipe"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[vbuf_layout],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_transparent"),
            compilation_options: Default::default(),
            targets: &transparent_targets,
        }),
        // Back-face cull water SIDE faces: otherwise a side face (e.g. an exposed
        // step over shallower water) shows its back as a dark sheet from the
        // water side, "in front of" the water that is actually there. The TOP
        // face is emitted in BOTH windings by the mesher, so the surface stays
        // visible from underneath (looking up while submerged) even with culling.
        primitive: wgpu::PrimitiveState {
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: sample_count,
            ..Default::default()
        },
        multiview: None,
        cache: None,
    });

    // --- Sky-background pipeline. ---
    // Uses its own Uniforms-only bind group because the sky shader does not need
    // atlas resources or the block pipeline's uv-rect table.
    let sky_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("sky bgl"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: NonZeroU64::new(std::mem::size_of::<Uniforms>() as u64),
            },
            count: None,
        }],
    });
    let sky_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("sky bg"),
        layout: &sky_bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform_buf.as_entire_binding(),
        }],
    });
    let sky_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("sky layout"),
        bind_group_layouts: &[&sky_bgl],
        push_constant_ranges: &[],
    });
    let sky_targets = [Some(wgpu::ColorTargetState {
        format,
        blend: Some(wgpu::BlendState::REPLACE),
        write_mask: wgpu::ColorWrites::ALL,
    })];
    let sky_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("sky pipe"),
        layout: Some(&sky_layout),
        vertex: wgpu::VertexState {
            module: &sky_shader,
            entry_point: Some("vs_sky"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &sky_shader,
            entry_point: Some("fs_sky"),
            compilation_options: Default::default(),
            targets: &sky_targets,
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState {
            count: sample_count,
            ..Default::default()
        },
        multiview: None,
        cache: None,
    });

    // --- Selection-outline pipeline. ---
    // Its own minimal bind-group layout (Uniforms at binding 0 only) so it
    // doesn't couple to the block pipelines' uv_rects layout. Reuses the same
    // uniform buffer for view_proj.
    let outline_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("outline shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/outline.wgsl").into()),
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
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform_buf.as_entire_binding(),
        }],
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
            format: wgpu::VertexFormat::Float32x3,
            offset: 0,
            shader_location: 0,
        }],
    };
    let outline_targets = [Some(wgpu::ColorTargetState {
        format,
        blend: Some(wgpu::BlendState::REPLACE),
        write_mask: wgpu::ColorWrites::ALL,
    })];
    let outline_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("outline pipe"),
        layout: Some(&outline_layout),
        vertex: wgpu::VertexState {
            module: &outline_shader,
            entry_point: Some("vs_outline"),
            compilation_options: Default::default(),
            buffers: &[outline_vbuf_layout],
        },
        fragment: Some(wgpu::FragmentState {
            module: &outline_shader,
            entry_point: Some("fs_outline"),
            compilation_options: Default::default(),
            targets: &outline_targets,
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
        multisample: wgpu::MultisampleState {
            count: sample_count,
            ..Default::default()
        },
        multiview: None,
        cache: None,
    });
    // Selection outline vertices x vec3<f32>.
    let outline_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("outline vbuf"),
        size: (MAX_OUTLINE_VERTICES * 12) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    // --- Center crosshair pipeline. ---
    // The fragment shader outputs white and the color blend computes
    // `white * (1 - dst) + dst * 0`, which inverts the pixels under the
    // crosshair instead of drawing a fixed light/dark color.
    let crosshair_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("crosshair layout"),
        bind_group_layouts: &[],
        push_constant_ranges: &[],
    });
    let crosshair_vbuf_layout = wgpu::VertexBufferLayout {
        array_stride: 8, // vec2<f32>
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 0,
            shader_location: 0,
        }],
    };
    let invert_blend = wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::OneMinusDst,
            dst_factor: wgpu::BlendFactor::Zero,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
    };
    let crosshair_targets = [Some(wgpu::ColorTargetState {
        format,
        blend: Some(invert_blend),
        write_mask: wgpu::ColorWrites::COLOR,
    })];
    let crosshair_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("crosshair pipe"),
        layout: Some(&crosshair_layout),
        vertex: wgpu::VertexState {
            module: &crosshair_shader,
            entry_point: Some("vs_crosshair"),
            compilation_options: Default::default(),
            buffers: &[crosshair_vbuf_layout],
        },
        fragment: Some(wgpu::FragmentState {
            module: &crosshair_shader,
            entry_point: Some("fs_crosshair"),
            compilation_options: Default::default(),
            targets: &crosshair_targets,
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState {
            count: sample_count,
            ..Default::default()
        },
        multiview: None,
        cache: None,
    });
    let crosshair_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("crosshair vbuf"),
        size: (MAX_CROSSHAIR_VERTICES * 8) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    // --- model3d pipeline (isometric slot icons + first-person held block). ---
    // group(0): a per-draw MVP mat4 via a DYNAMIC-OFFSET uniform (binding 0) plus
    // the shared uv_rects table (binding 1, same as the block pipeline). group(1):
    // the block atlas (reuse the atlas bgl shape). Full-bright, back-face culled,
    // alpha-blended so flat sprite items cut out. Built in TWO depth variants from
    // the SAME shader/layout: `model3d_pipe` (NO depth) for the depthless UI icon
    // pass, and `model3d_hand_pipe` (depth test + write) for the hand pass, which
    // now carries a cleared depth buffer so the held block self-sorts.
    let model3d_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("model3d shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/model3d.wgsl").into()),
    });
    let model3d_mvp_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("model3d mvp bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    // One mat4 per draw (64 bytes); slots are 256-aligned.
                    min_binding_size: NonZeroU64::new(64),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new((UV_RECTS_LEN * 16) as u64),
                },
                count: None,
            },
        ],
    });
    let model3d_mvp_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("model3d mvp"),
        size: MODEL3D_MVP_SLOTS * MODEL3D_MVP_SLOT_SIZE,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let model3d_mvp_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("model3d mvp bg"),
        layout: &model3d_mvp_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                // Bound as a 64-byte mat4 window; the per-draw offset selects the slot.
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &model3d_mvp_buf,
                    offset: 0,
                    size: NonZeroU64::new(64),
                }),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: uv_rects_buf.as_entire_binding(),
            },
        ],
    });
    let model3d_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("model3d layout"),
        bind_group_layouts: &[&model3d_mvp_bgl, &atlas_bgl],
        push_constant_ranges: &[],
    });
    let model3d_vbuf_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<Vertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &vbuf_attrs,
    };
    let model3d_targets = [Some(wgpu::ColorTargetState {
        format,
        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
        write_mask: wgpu::ColorWrites::ALL,
    })];
    let model3d_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("model3d pipe"),
        layout: Some(&model3d_layout),
        vertex: wgpu::VertexState {
            module: &model3d_shader,
            entry_point: Some("vs_model"),
            compilation_options: Default::default(),
            buffers: std::slice::from_ref(&model3d_vbuf_layout),
        },
        fragment: Some(wgpu::FragmentState {
            module: &model3d_shader,
            entry_point: Some("fs_model"),
            compilation_options: Default::default(),
            targets: &model3d_targets,
        }),
        primitive: wgpu::PrimitiveState {
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        },
        // No depth: the iso icons are drawn in the depthless UI pass.
        depth_stencil: None,
        multisample: wgpu::MultisampleState {
            count: sample_count,
            ..Default::default()
        },
        multiview: None,
        cache: None,
    });
    // Hand-pass variant: identical shader/layout/blend/cull, but WITH a depth
    // attachment so the first-person held block self-sorts against the hand pass's
    // cleared depth buffer (a single pipeline cannot serve both the depthless UI
    // icon pass and the depth-attached hand pass).
    let model3d_hand_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("model3d hand pipe"),
        layout: Some(&model3d_layout),
        vertex: wgpu::VertexState {
            module: &model3d_shader,
            entry_point: Some("vs_model"),
            compilation_options: Default::default(),
            buffers: std::slice::from_ref(&model3d_vbuf_layout),
        },
        fragment: Some(wgpu::FragmentState {
            module: &model3d_shader,
            entry_point: Some("fs_model"),
            compilation_options: Default::default(),
            targets: &model3d_targets,
        }),
        primitive: wgpu::PrimitiveState {
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: sample_count,
            ..Default::default()
        },
        multiview: None,
        cache: None,
    });
    let model3d_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("model3d vbuf"),
        size: MAX_MODEL3D_VERTICES * std::mem::size_of::<Vertex>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let model3d_ibuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("model3d ibuf"),
        size: MAX_MODEL3D_INDICES * 4,
        usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    // --- item3d pipeline (extruded first-person held item). ---
    // group(0) = a per-draw MVP via a DYNAMIC-OFFSET uniform (binding 0) over the
    // shared `model3d_mvp_buf` (reuses its 256-byte-slot pattern). group(1) = the
    // block atlas (reuse the atlas bgl). Explicit per-vertex (pos, uv, shade) so
    // the side walls can sample a single boundary texel's sub-UV (the model3d
    // packed-vertex shader can only SELECT whole-tile UV corners). Full-bright,
    // alpha-cutout, DOUBLE-SIDED (cull off so the back face + inner walls show),
    // NO depth (drawn over the world in the hand pass), alpha-blended.
    let item3d_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("item3d shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/item3d.wgsl").into()),
    });
    let item3d_mvp_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("item3d mvp bgl"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: true,
                // One mat4 per draw (64 bytes); slots are 256-aligned.
                min_binding_size: NonZeroU64::new(64),
            },
            count: None,
        }],
    });
    let item3d_mvp_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("item3d mvp bg"),
        layout: &item3d_mvp_bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &model3d_mvp_buf,
                offset: 0,
                size: NonZeroU64::new(64),
            }),
        }],
    });
    let item3d_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("item3d layout"),
        bind_group_layouts: &[&item3d_mvp_bgl, &atlas_bgl],
        push_constant_ranges: &[],
    });
    // Vertex: pos (f32x3 @0) + uv (f32x2 @12) + shade (f32 @20) = 24 bytes.
    let item3d_vbuf_attrs = [
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x3,
            offset: 0,
            shader_location: 0,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 12,
            shader_location: 1,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32,
            offset: 20,
            shader_location: 2,
        },
    ];
    let item3d_vbuf_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<super::item_model::ItemVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &item3d_vbuf_attrs,
    };
    let item3d_targets = [Some(wgpu::ColorTargetState {
        format,
        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
        write_mask: wgpu::ColorWrites::ALL,
    })];
    let item3d_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("item3d pipe"),
        layout: Some(&item3d_layout),
        vertex: wgpu::VertexState {
            module: &item3d_shader,
            entry_point: Some("vs_item"),
            compilation_options: Default::default(),
            buffers: std::slice::from_ref(&item3d_vbuf_layout),
        },
        fragment: Some(wgpu::FragmentState {
            module: &item3d_shader,
            entry_point: Some("fs_item"),
            compilation_options: Default::default(),
            targets: &item3d_targets,
        }),
        // Double-sided: the back face + inward walls must never cull.
        primitive: wgpu::PrimitiveState {
            cull_mode: None,
            ..Default::default()
        },
        // Depth-test + write against the hand pass's own (cleared) depth buffer so
        // the extruded mesh self-sorts: front, stepped side walls and back no longer
        // overdraw each other in submission order. The hand pass clears depth, so
        // this stays isolated from the world (the item still draws over terrain).
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: sample_count,
            ..Default::default()
        },
        multiview: None,
        cache: None,
    });
    let item3d_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("item3d vbuf"),
        size: MAX_ITEM3D_VERTICES * std::mem::size_of::<super::item_model::ItemVertex>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    // --- Break-overlay pipeline (the destroy crack). ---
    // Reuses the block `uniform_bgl` (group0: view_proj + uv_rects) + `atlas_bgl`
    // (group1) so it binds the renderer's existing `uniform_bind` / `atlas_bind`
    // unchanged. Same 28-byte vertex as the block pipe. Alpha-blended; depth
    // LessEqual / no-write; the cube is CPU-inflated to win the depth tie.
    let break_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("break overlay shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/break_overlay.wgsl").into()),
    });
    let break_vbuf_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<Vertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &vbuf_attrs,
    };
    // MULTIPLY blend (result = src.rgb * dst.rgb): the crack fragment outputs WHITE
    // where the destroy tile is transparent (×1 = no change) and dark where the
    // crack texels are, so the cracks darken the block face instead of
    // alpha-compositing a flat overlay. `color = Dst * src + Zero * dst = src*dst`.
    // Alpha is preserved (Zero/One) — the colour target keeps its existing alpha.
    let multiply_blend = wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Dst,
            dst_factor: wgpu::BlendFactor::Zero,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
    };
    let break_targets = [Some(wgpu::ColorTargetState {
        format,
        blend: Some(multiply_blend),
        write_mask: wgpu::ColorWrites::ALL,
    })];
    // group0 = block uniform layout (Uniforms + uv_rects); group1 = atlas. Same
    // layout object as the opaque/transparent pipes (`layout`).
    let break_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("break overlay pipe"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &break_shader,
            entry_point: Some("vs_break"),
            compilation_options: Default::default(),
            buffers: std::slice::from_ref(&break_vbuf_layout),
        },
        fragment: Some(wgpu::FragmentState {
            module: &break_shader,
            entry_point: Some("fs_break"),
            compilation_options: Default::default(),
            targets: &break_targets,
        }),
        primitive: wgpu::PrimitiveState {
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::LessEqual,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: sample_count,
            ..Default::default()
        },
        multiview: None,
        cache: None,
    });
    let break_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("break vbuf"),
        size: MAX_BREAK_VERTICES * std::mem::size_of::<Vertex>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let break_ibuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("break ibuf"),
        size: MAX_BREAK_INDICES * 4,
        usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    // Item-entity dynamic buffers (drawn by the opaque pipeline; separate from the
    // hand's model3d buffers so the hand pass doesn't clobber them).
    let item_entity_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("item entity vbuf"),
        size: MAX_ITEM_ENTITY_VERTICES * std::mem::size_of::<Vertex>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let item_entity_ibuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("item entity ibuf"),
        size: MAX_ITEM_ENTITY_INDICES * 4,
        usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    // --- Particle pipeline (tiny 3D textured cubes). ---
    // Reuses the block `uniform_bgl` (group0) + `atlas_bgl` (group1) so it binds
    // the renderer's existing `uniform_bind` / `atlas_bind`. Compact 40-byte
    // particle vertex (pos + uv + tint + shade + alpha). Alpha CUTOUT (discard
    // a<0.5 in the shader) so cubes are solid and DEPTH-WRITTEN — correctly
    // occluded by terrain and visible from any angle including above. No blend.
    let particle_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("particle shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/particles.wgsl").into()),
    });
    let particle_vbuf_attrs = [
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x3,
            offset: 0,
            shader_location: 0,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 12,
            shader_location: 1,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x3,
            offset: 20,
            shader_location: 2,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32,
            offset: 32,
            shader_location: 3,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32,
            offset: 36,
            shader_location: 4,
        },
    ];
    let particle_vbuf_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<super::particles::ParticleVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &particle_vbuf_attrs,
    };
    // Opaque cubes (cutout discard handles transparency) — no blend.
    let particle_targets = [Some(wgpu::ColorTargetState {
        format,
        blend: None,
        write_mask: wgpu::ColorWrites::ALL,
    })];
    let particle_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("particle pipe"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &particle_shader,
            entry_point: Some("vs_particle"),
            compilation_options: Default::default(),
            buffers: std::slice::from_ref(&particle_vbuf_layout),
        },
        fragment: Some(wgpu::FragmentState {
            module: &particle_shader,
            entry_point: Some("fs_particle"),
            compilation_options: Default::default(),
            targets: &particle_targets,
        }),
        // Cubes carry their own per-face winding; disabling cull is robust (and the
        // cutout discard means we never rely on backface rejection for the look).
        primitive: wgpu::PrimitiveState {
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: sample_count,
            ..Default::default()
        },
        multiview: None,
        cache: None,
    });
    let particle_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("particle vbuf"),
        size: (super::particles::MAX_PARTICLE_VERTICES
            * std::mem::size_of::<super::particles::ParticleVertex>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    // Static quad indices, uploaded once (only the vbuf is rewritten per frame).
    let particle_ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("particle ibuf"),
        contents: bytemuck::cast_slice(&super::particles::particle_indices()),
        usage: wgpu::BufferUsages::INDEX,
    });

    // --- UI pipeline (2D HUD / inventory). ---
    // group(0) is the SEPARATE gui sprite atlas (texture + sampler) — NOT the
    // block atlas. Vertices are NDC pos (vec2) + uv (vec2) + color (vec4); the
    // fragment shader outputs the vertex color for the solid sentinel (uv.x < 0)
    // and otherwise samples the gui atlas * color. Alpha-blended, NO depth, drawn
    // LAST so it sits over every world / hand / crosshair pass.
    let ui_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("ui shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/ui.wgsl").into()),
    });
    let ui_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ui bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let ui_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("ui bg"),
        layout: &ui_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(gui_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(gui_sampler),
            },
        ],
    });
    let ui_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("ui layout"),
        bind_group_layouts: &[&ui_bgl],
        push_constant_ranges: &[],
    });
    let ui_vbuf_attrs = [
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 0,
            shader_location: 0,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 8,
            shader_location: 1,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x4,
            offset: 16,
            shader_location: 2,
        },
    ];
    let ui_vbuf_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<super::ui::UiVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &ui_vbuf_attrs,
    };
    let ui_targets = [Some(wgpu::ColorTargetState {
        format,
        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
        write_mask: wgpu::ColorWrites::ALL,
    })];
    let ui_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("ui pipe"),
        layout: Some(&ui_layout),
        vertex: wgpu::VertexState {
            module: &ui_shader,
            entry_point: Some("vs_ui"),
            compilation_options: Default::default(),
            buffers: std::slice::from_ref(&ui_vbuf_layout),
        },
        fragment: Some(wgpu::FragmentState {
            module: &ui_shader,
            entry_point: Some("fs_ui"),
            compilation_options: Default::default(),
            targets: &ui_targets,
        }),
        // UI quads are CPU-emitted CCW but disabling cull is robust against either
        // winding; no depth (last pass).
        primitive: wgpu::PrimitiveState {
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState {
            count: sample_count,
            ..Default::default()
        },
        multiview: None,
        cache: None,
    });
    let ui_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ui vbuf"),
        size: MAX_UI_VERTICES * std::mem::size_of::<super::ui::UiVertex>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    PipelineResources {
        uniform_bind,
        atlas_bind,
        sky_pipe,
        sky_bind,
        opaque_pipe,
        transparent_pipe,
        outline_pipe,
        outline_bind,
        outline_vbuf,
        crosshair_pipe,
        crosshair_vbuf,
        model3d_pipe,
        model3d_hand_pipe,
        model3d_mvp_buf,
        model3d_mvp_bind,
        model3d_vbuf,
        model3d_ibuf,
        item3d_pipe,
        item3d_mvp_bind,
        item3d_vbuf,
        break_pipe,
        break_vbuf,
        break_ibuf,
        item_entity_vbuf,
        item_entity_ibuf,
        particle_pipe,
        particle_vbuf,
        particle_ibuf,
        ui_pipe,
        ui_bind,
        ui_vbuf,
    }
}

#[cfg(test)]
mod gpu_validation {
    use super::*;
    use crate::render::instance_descriptor;

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
        let adapter =
            match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: false,
            })) {
                Ok(a) => a,
                Err(_) => {
                    eprintln!("[skip] no wgpu adapter; pipeline validation not run");
                    return;
                }
            };
        let (device, _queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: None,
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default().using_alignment(adapter.limits()),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            }))
            .expect("device");

        device.push_error_scope(wgpu::ErrorFilter::Validation);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("block shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/block.wgsl").into()),
        });

        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(std::mem::size_of::<Uniforms>() as u64),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new((UV_RECTS_LEN * 16) as u64),
                    },
                    count: None,
                },
            ],
        });
        let atlas_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&uniform_bgl, &atlas_bgl],
            push_constant_ranges: &[],
        });

        let vbuf_attrs = [
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 0,
                shader_location: 0,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 12,
                shader_location: 1,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Uint32,
                offset: 24,
                shader_location: 2,
            },
        ];
        let vbuf_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &vbuf_attrs,
        };
        let targets = [Some(wgpu::ColorTargetState {
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            blend: Some(wgpu::BlendState::REPLACE),
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let _pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[vbuf_layout],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_opaque"),
                compilation_options: Default::default(),
                targets: &targets,
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // Also validate the outline pipeline + shader (LineList, group0 = a
        // minimal Uniforms-only bind group).
        let outline_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("outline shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/outline.wgsl").into()),
        });
        let outline_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
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
        let outline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&outline_bgl],
            push_constant_ranges: &[],
        });
        let outline_vbuf_layout = wgpu::VertexBufferLayout {
            array_stride: 12,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 0,
                shader_location: 0,
            }],
        };
        let _outline_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&outline_layout),
            vertex: wgpu::VertexState {
                module: &outline_shader,
                entry_point: Some("vs_outline"),
                compilation_options: Default::default(),
                buffers: &[outline_vbuf_layout],
            },
            fragment: Some(wgpu::FragmentState {
                module: &outline_shader,
                entry_point: Some("fs_outline"),
                compilation_options: Default::default(),
                targets: &targets,
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // Validate the fullscreen sky pipeline too. It uses the same Uniforms
        // layout but no vertex buffers, atlas resources, or depth attachment.
        let sky_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sky shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/sky.wgsl").into()),
        });
        let sky_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(std::mem::size_of::<Uniforms>() as u64),
                },
                count: None,
            }],
        });
        let sky_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&sky_bgl],
            push_constant_ranges: &[],
        });
        let _sky_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&sky_layout),
            vertex: wgpu::VertexState {
                module: &sky_shader,
                entry_point: Some("vs_sky"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &sky_shader,
                entry_point: Some("fs_sky"),
                compilation_options: Default::default(),
                targets: &targets,
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // Validate the crosshair pipeline, including the destination-color blend
        // used to invert the pixels under the crosshair.
        let crosshair_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("crosshair shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/crosshair.wgsl").into()),
        });
        let crosshair_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[],
            push_constant_ranges: &[],
        });
        let crosshair_vbuf_layout = wgpu::VertexBufferLayout {
            array_stride: 8,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x2,
                offset: 0,
                shader_location: 0,
            }],
        };
        let crosshair_targets = [Some(wgpu::ColorTargetState {
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            blend: Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::OneMinusDst,
                    dst_factor: wgpu::BlendFactor::Zero,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::Zero,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
            }),
            write_mask: wgpu::ColorWrites::COLOR,
        })];
        let _crosshair_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&crosshair_layout),
            vertex: wgpu::VertexState {
                module: &crosshair_shader,
                entry_point: Some("vs_crosshair"),
                compilation_options: Default::default(),
                buffers: &[crosshair_vbuf_layout],
            },
            fragment: Some(wgpu::FragmentState {
                module: &crosshair_shader,
                entry_point: Some("fs_crosshair"),
                compilation_options: Default::default(),
                targets: &crosshair_targets,
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // Validate the model3d pipeline + shader: group0 = a dynamic-offset MVP
        // mat4 (binding 0) + the uv_rects table (binding 1); group1 = the block
        // atlas (texture + sampler). Same 28-byte vertex layout as the block pipe,
        // back-face cull, alpha blend. Built in two depth variants (depthless UI
        // icons + the depth-enabled hand block) — both validated below.
        let model3d_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("model3d shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/model3d.wgsl").into()),
        });
        let model3d_mvp_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: NonZeroU64::new(64),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new((UV_RECTS_LEN * 16) as u64),
                    },
                    count: None,
                },
            ],
        });
        let model3d_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&model3d_mvp_bgl, &atlas_bgl],
            push_constant_ranges: &[],
        });
        let model3d_vbuf_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &vbuf_attrs,
        };
        let model3d_targets = [Some(wgpu::ColorTargetState {
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            blend: Some(wgpu::BlendState::ALPHA_BLENDING),
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let _model3d_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&model3d_layout),
            vertex: wgpu::VertexState {
                module: &model3d_shader,
                entry_point: Some("vs_model"),
                compilation_options: Default::default(),
                buffers: std::slice::from_ref(&model3d_vbuf_layout),
            },
            fragment: Some(wgpu::FragmentState {
                module: &model3d_shader,
                entry_point: Some("fs_model"),
                compilation_options: Default::default(),
                targets: &model3d_targets,
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        // Depth-enabled hand variant of model3d (same shader/layout, depth Less +
        // write): used in the hand pass, which clears depth so the held block
        // self-sorts.
        let _model3d_hand_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&model3d_layout),
            vertex: wgpu::VertexState {
                module: &model3d_shader,
                entry_point: Some("vs_model"),
                compilation_options: Default::default(),
                buffers: &[model3d_vbuf_layout],
            },
            fragment: Some(wgpu::FragmentState {
                module: &model3d_shader,
                entry_point: Some("fs_model"),
                compilation_options: Default::default(),
                targets: &model3d_targets,
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // Validate the item3d pipeline + shader: group0 = a dynamic-offset MVP
        // mat4 (binding 0 only); group1 = the block atlas (texture + sampler).
        // 24-byte vertex (pos f32x3 + uv f32x2 + shade f32), double-sided (no
        // cull), depth test + write (the hand pass clears depth so the extruded
        // mesh self-sorts), alpha blend.
        let item3d_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("item3d shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/item3d.wgsl").into()),
        });
        let item3d_mvp_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: NonZeroU64::new(64),
                },
                count: None,
            }],
        });
        let item3d_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&item3d_mvp_bgl, &atlas_bgl],
            push_constant_ranges: &[],
        });
        let item3d_vbuf_attrs = [
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 0,
                shader_location: 0,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x2,
                offset: 12,
                shader_location: 1,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: 20,
                shader_location: 2,
            },
        ];
        let item3d_vbuf_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<crate::render::item_model::ItemVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &item3d_vbuf_attrs,
        };
        let item3d_targets = [Some(wgpu::ColorTargetState {
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            blend: Some(wgpu::BlendState::ALPHA_BLENDING),
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let _item3d_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&item3d_layout),
            vertex: wgpu::VertexState {
                module: &item3d_shader,
                entry_point: Some("vs_item"),
                compilation_options: Default::default(),
                buffers: &[item3d_vbuf_layout],
            },
            fragment: Some(wgpu::FragmentState {
                module: &item3d_shader,
                entry_point: Some("fs_item"),
                compilation_options: Default::default(),
                targets: &item3d_targets,
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // Validate the break-overlay pipeline + shader: group0 = the block
        // uniform layout (Uniforms + uv_rects), group1 = the atlas; 28-byte vertex,
        // alpha blend, depth LessEqual / no-write.
        let break_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("break overlay shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/break_overlay.wgsl").into()),
        });
        let break_vbuf_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &vbuf_attrs,
        };
        // MULTIPLY blend (result = src.rgb * dst.rgb): cracks darken the face. Must
        // match the runtime `break_pipe` blend so this validates the real pipeline.
        let break_targets = [Some(wgpu::ColorTargetState {
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            blend: Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::Dst,
                    dst_factor: wgpu::BlendFactor::Zero,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::Zero,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
            }),
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let _break_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &break_shader,
                entry_point: Some("vs_break"),
                compilation_options: Default::default(),
                buffers: &[break_vbuf_layout],
            },
            fragment: Some(wgpu::FragmentState {
                module: &break_shader,
                entry_point: Some("fs_break"),
                compilation_options: Default::default(),
                targets: &break_targets,
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // Validate the particle pipeline + shader: same group0/group1 layout,
        // compact 40-byte particle vertex (pos + uv + tint + shade + alpha), NO
        // blend (cutout discard), depth test (Less) WITH depth-write, no cull.
        let particle_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("particle shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/particles.wgsl").into()),
        });
        let particle_vbuf_attrs = [
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 0,
                shader_location: 0,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x2,
                offset: 12,
                shader_location: 1,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 20,
                shader_location: 2,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: 32,
                shader_location: 3,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: 36,
                shader_location: 4,
            },
        ];
        let particle_vbuf_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<crate::render::particles::ParticleVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &particle_vbuf_attrs,
        };
        let particle_targets = [Some(wgpu::ColorTargetState {
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            blend: None,
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let _particle_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &particle_shader,
                entry_point: Some("vs_particle"),
                compilation_options: Default::default(),
                buffers: &[particle_vbuf_layout],
            },
            fragment: Some(wgpu::FragmentState {
                module: &particle_shader,
                entry_point: Some("fs_particle"),
                compilation_options: Default::default(),
                targets: &particle_targets,
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // Validate the UI pipeline + shader: group0 = the gui sprite atlas
        // (texture + sampler); 32-byte vertex (NDC pos vec2 + uv vec2 + color
        // vec4), alpha blend, no depth, no cull.
        let ui_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ui shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/ui.wgsl").into()),
        });
        let ui_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let ui_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&ui_bgl],
            push_constant_ranges: &[],
        });
        let ui_vbuf_attrs = [
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x2,
                offset: 0,
                shader_location: 0,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x2,
                offset: 8,
                shader_location: 1,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 16,
                shader_location: 2,
            },
        ];
        let ui_vbuf_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<crate::render::ui::UiVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &ui_vbuf_attrs,
        };
        let ui_targets = [Some(wgpu::ColorTargetState {
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            blend: Some(wgpu::BlendState::ALPHA_BLENDING),
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let _ui_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&ui_layout),
            vertex: wgpu::VertexState {
                module: &ui_shader,
                entry_point: Some("vs_ui"),
                compilation_options: Default::default(),
                buffers: &[ui_vbuf_layout],
            },
            fragment: Some(wgpu::FragmentState {
                module: &ui_shader,
                entry_point: Some("fs_ui"),
                compilation_options: Default::default(),
                targets: &ui_targets,
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let err = pollster::block_on(device.pop_error_scope());
        assert!(
            err.is_none(),
            "packed-vertex pipeline validation error: {err:?}"
        );
        // Confirm the assumption baked into the packing: tile ids fit in 8 bits.
        const { assert!(TILE_COUNT <= 256) };
        // Stride sanity: the compressed vertex is exactly 28 bytes.
        assert_eq!(std::mem::size_of::<Vertex>(), 28);
        // item3d vertex stride must match its declared attribute layout (24).
        assert_eq!(
            std::mem::size_of::<crate::render::item_model::ItemVertex>(),
            24
        );
    }
}
