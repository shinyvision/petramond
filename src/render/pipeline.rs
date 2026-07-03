use crate::atlas::{tile_uv, Tile};
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
/// Max vertices / indices in the reusable `mob` dynamic buffers (animated entity
/// models), drawn by the dedicated mob pipeline with the explicit-UV `ItemVertex`.
/// Current mobs are well under this; it covers dozens of simultaneously-visible
/// instances before a bake bails for that frame.
pub(super) const MAX_MOB_VERTICES: u64 = 20480;
pub(super) const MAX_MOB_INDICES: u64 = 30720;
/// Boxes the break-overlay buffers must hold: a legacy block cracks over ONE cube, but a
/// bbmodel block cracks over EVERY cube of its model (the workbench is ~36), so size for a
/// comfortably complex model — otherwise the multi-box bake overflows and the whole crack
/// silently vanishes (the bug this fixes).
pub(super) const MAX_BREAK_BOXES: u64 = 256;
/// Vertices in the break-overlay dynamic vbuf (24 per box).
pub(super) const MAX_BREAK_VERTICES: u64 = MAX_BREAK_BOXES * 24;
/// Indices in the break-overlay dynamic ibuf (36 per box).
pub(super) const MAX_BREAK_INDICES: u64 = MAX_BREAK_BOXES * 36;
/// Polygon offset for the break-overlay decal: nudge the crack toward the camera
/// (depth is standard near=0/far=1, so negative = closer) so it reliably wins the
/// `LessEqual` depth tie against the coincident block face despite the mesher's
/// per-AO triangulation flip. The `constant` term covers head-on faces (depth slope
/// ~0); the `slope_scale` term covers glancing angles. Mirrors Minecraft's
/// crumbling/break layer offset (`polygonOffset(-1.0, -10.0)`): a few ULP — far too
/// small to overcome a genuinely closer surface or to read as parallax.
const BREAK_DEPTH_BIAS: wgpu::DepthBiasState = wgpu::DepthBiasState {
    constant: -10,
    slope_scale: -1.0,
    clamp: 0.0,
};

// The break-overlay crack cube is COINCIDENT with the block faces, so it wins the
// depth `LessEqual` tie only via a polygon offset toward the camera. Depth is
// standard (near=0/far=1, closer = smaller), so both offset terms MUST be negative
// — a positive or zero bias would leave the decal at/behind the surface and the
// crack would z-fight or vanish. Guard the sign at COMPILE TIME so a future
// "cleanup" can't silently break it. (The magnitude is intentionally unchecked: the
// float-depth bias unit is implementation-defined per the WebGPU/Vulkan spec.)
const _: () = assert!(
    BREAK_DEPTH_BIAS.constant < 0,
    "constant bias must be negative (toward camera)"
);
const _: () = assert!(
    BREAK_DEPTH_BIAS.slope_scale < 0.0,
    "slope-scaled bias must be negative (toward camera)"
);

/// The render target's depth format. Every depth-tested pass shares one
/// `Depth32Float` attachment, so the presets below all use this.
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// A named depth-stencil configuration for [`world_pipeline`]. The world passes
/// only ever vary along three axes — whether depth is written, the compare
/// function, and the polygon-offset bias — so each variant captures one real
/// combination instead of re-spelling the `DepthStencilState` block per pipeline.
#[derive(Copy, Clone)]
enum DepthPreset {
    /// Depth test `Less` + WRITE. Opaque geometry, particles, and the hand
    /// variants that self-sort against a cleared depth buffer.
    WriteLess,
    /// Depth test `Less`, NO write. Transparent water: sorts behind solid
    /// geometry without occluding the surfaces drawn after it.
    ReadLess,
    /// Depth test `LessEqual`, NO write, with the break-overlay polygon offset.
    /// The crack decal is coincident with the block faces; the bias wins the tie.
    ReadLessEqualBiased,
    /// Depth test `LessEqual`, NO write, no bias. The selection outline: hidden
    /// behind terrain but its slightly-inflated front edges win the equal test.
    ReadLessEqual,
}

impl DepthPreset {
    fn state(self) -> wgpu::DepthStencilState {
        let (write, compare, bias) = match self {
            DepthPreset::WriteLess => (
                true,
                wgpu::CompareFunction::Less,
                wgpu::DepthBiasState::default(),
            ),
            DepthPreset::ReadLess => (
                false,
                wgpu::CompareFunction::Less,
                wgpu::DepthBiasState::default(),
            ),
            DepthPreset::ReadLessEqualBiased => {
                (false, wgpu::CompareFunction::LessEqual, BREAK_DEPTH_BIAS)
            }
            DepthPreset::ReadLessEqual => (
                false,
                wgpu::CompareFunction::LessEqual,
                wgpu::DepthBiasState::default(),
            ),
        };
        wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: write,
            depth_compare: compare,
            stencil: wgpu::StencilState::default(),
            bias,
        }
    }
}

/// One color target with the given blend. `write_mask` is `ALL` for every pass
/// except the crosshair (which writes COLOR only); pass that explicitly.
fn color_target(
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
    write_mask: wgpu::ColorWrites,
) -> [Option<wgpu::ColorTargetState>; 1] {
    [Some(wgpu::ColorTargetState {
        format,
        blend,
        write_mask,
    })]
}

/// Build a render pipeline, filling the fields that are constant across every
/// pass in this module (`compilation_options`, the shared `sample_count`
/// multisample state, `multiview: None`, `cache: None`) exactly once. Callers
/// supply only what actually varies per pass: label, layout, shader + entry
/// points, vertex buffer layouts, the color targets, the primitive state, and an
/// optional [`DepthPreset`] (`None` = no depth attachment).
///
/// Vertex and fragment stages share one `shader` module — every pass in this
/// file does. The depth-less UI / icon passes pass `depth: None`; that is the
/// ONLY difference between e.g. `model3d_pipe` and `model3d_hand_pipe`.
#[allow(clippy::too_many_arguments)]
fn world_pipeline(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    vs_entry: &str,
    fs_entry: &str,
    buffers: &[wgpu::VertexBufferLayout],
    targets: &[Option<wgpu::ColorTargetState>],
    primitive: wgpu::PrimitiveState,
    depth: Option<DepthPreset>,
    sample_count: u32,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(vs_entry),
            compilation_options: Default::default(),
            buffers,
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fs_entry),
            compilation_options: Default::default(),
            targets,
        }),
        primitive,
        depth_stencil: depth.map(DepthPreset::state),
        multisample: wgpu::MultisampleState {
            count: sample_count,
            ..Default::default()
        },
        multiview: None,
        cache: None,
    })
}
/// Max vertices in the item-entity dynamic vbuf. A stack draws up to 5 layered
/// copies (120 verts per cube / 40 per sprite), so this is sized 5× the old
/// single-copy budget to still cover ~170 simultaneously-visible dropped items
/// (more when they're single, unstacked drops) without the bake overflowing and
/// dropping every item entity that frame.
pub(super) const MAX_ITEM_ENTITY_VERTICES: u64 = 20480;
/// Max indices in the item-entity dynamic ibuf (up to 180 per cube / 30 per
/// sprite for a 5-layer stack), matching [`MAX_ITEM_ENTITY_VERTICES`].
pub(super) const MAX_ITEM_ENTITY_INDICES: u64 = 30720;
/// Max vertices in the chest dynamic vbuf. Each chest is a body box + lid box = 48
/// verts, so this covers ~512 simultaneously-visible chests before the bake bails
/// for that frame. Separate from the item-entity budget so a wall of chests can't
/// make dropped items vanish.
pub(super) const MAX_CHEST_VERTICES: u64 = 24576;
/// Max indices in the chest dynamic ibuf (72 per chest), matching
/// [`MAX_CHEST_VERTICES`].
pub(super) const MAX_CHEST_INDICES: u64 = 36864;
/// Max vertices in the door dynamic vbuf. Each door is two boxes (lower + upper half)
/// = 48 verts, so this covers ~512 simultaneously-visible doors before the bake bails.
/// Separate from the chest budget so a wall of doors can't make chests vanish.
pub(super) const MAX_DOOR_VERTICES: u64 = 24576;
/// Max indices in the door dynamic ibuf (72 per door), matching [`MAX_DOOR_VERTICES`].
pub(super) const MAX_DOOR_INDICES: u64 = 36864;
/// Max vertices in each reusable UI dynamic vbuf (gui quads, stack-count digits,
/// icon quads, and text quads). Shell labels are drawn from runtime text atlases,
/// so this no longer needs to cover one solid quad per text bitmap cell.
pub(super) const MAX_UI_VERTICES: u64 = 16384;

pub(super) struct PipelineResources {
    pub uniform_bind: wgpu::BindGroup,
    pub atlas_bind: wgpu::BindGroup,
    /// The terrain tile-ARRAY bind (group 1 for the opaque/transparent block pipelines),
    /// parallel to `atlas_bind` — see [`create_atlas_array`](super::resources::create_atlas_array).
    pub atlas_array_bind: wgpu::BindGroup,
    /// The atlas bind-group LAYOUT (texture + sampler), returned so the renderer
    /// can build a separate bind group over the entity model texture for the `mob`
    /// pipeline (same shape, different texture).
    pub atlas_bgl: wgpu::BindGroupLayout,
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
    /// The model3d group(0) bind-group LAYOUT (dynamic-offset MVP at binding 0 +
    /// uv_rects at binding 1), exposed so the renderer can build a SEPARATE,
    /// icon-count-sized MVP buffer + bind for the one-time icon-atlas bake (Pass A
    /// needs one live MVP slot per cube/sprite icon simultaneously — more than the
    /// per-frame [`MODEL3D_MVP_SLOTS`]).
    pub model3d_mvp_bgl: wgpu::BindGroupLayout,
    /// The shared uv-rect table buffer (binding 1 of the model3d group(0)), exposed
    /// so the icon-atlas bake's own MVP bind group can reference the same table.
    pub uv_rects_buf: wgpu::Buffer,
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
    /// `mob` pipeline: in-world animated entity models. Reuses the block
    /// `uniform_bgl` + `atlas_bgl` pipeline layout (group0 = world `view_proj`,
    /// group1 = the ENTITY texture bound by the renderer), the explicit-UV
    /// `ItemVertex`, REPLACE blend + alpha-cutout, double-sided (flat sub-cubes show
    /// from both sides), depth test + WRITE so mobs occlude terrain.
    pub mob_pipe: wgpu::RenderPipeline,
    /// Break-overlay pipeline: the cracked-block destroy quad. Reuses the block
    /// `uniform_bind` (view_proj + uv_rects) + `atlas_bind`, alpha-blended, depth
    /// LessEqual / no-write over geometry coincident with the block faces.
    pub break_pipe: wgpu::RenderPipeline,
    /// Reusable dynamic vbuf for the break overlay (one block-sized cube).
    pub break_vbuf: wgpu::Buffer,
    /// Reusable dynamic ibuf for the break overlay (one cube).
    pub break_ibuf: wgpu::Buffer,
    /// Reusable dynamic vbuf for item-entity geometry (drawn by the opaque pipe).
    pub item_entity_vbuf: wgpu::Buffer,
    /// Reusable dynamic ibuf for item-entity geometry.
    pub item_entity_ibuf: wgpu::Buffer,
    /// Reusable dynamic vbuf for chest models (body + hinged lid, opaque pipe).
    pub chest_vbuf: wgpu::Buffer,
    /// Reusable dynamic ibuf for chest models.
    pub chest_ibuf: wgpu::Buffer,
    /// Reusable dynamic vbuf for door models (2-tall hinged slab, opaque pipe).
    pub door_vbuf: wgpu::Buffer,
    /// Reusable dynamic ibuf for door models.
    pub door_ibuf: wgpu::Buffer,
    /// Particle pipeline: camera-facing billboards. Reuses the block `uniform_bind`
    /// + `atlas_bind`, alpha-blended, depth-test (Load) / no-write.
    pub particle_pipe: wgpu::RenderPipeline,
    /// Reusable dynamic vbuf for particle quads (rewritten in place per frame).
    pub particle_vbuf: wgpu::Buffer,
    /// Static ibuf for particle quads (6 per quad), uploaded once.
    pub particle_ibuf: wgpu::Buffer,
    /// UI pipeline: 2D HUD / inventory quads (NDC pos + uv + color). Alpha-blended,
    /// NO depth, drawn last; group(0) binds whatever texture each quad samples — a
    /// baked GUI texture or the icon atlas (solid quads ignore the sampler).
    pub ui_pipe: wgpu::RenderPipeline,
    /// Reusable dynamic vbuf for the UI's solid quads (dim backdrop + digits).
    pub ui_vbuf: wgpu::Buffer,
    /// model-icon pipeline: bbmodel-block icons. The icon MVP is baked into the
    /// `ItemVertex` positions CPU-side and the faces self-sort by depth (the model is
    /// double-sided like the in-world block), so this is a near pass-through sampling
    /// the MODEL atlas at group(0); alpha-cutout, alpha-blended, depth test + write.
    /// Used ONLY to bake the bbmodel-block cells of the icon atlas at renderer init.
    pub model_icon_pipe: wgpu::RenderPipeline,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn create_pipeline_resources(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    sample_count: u32,
    uniform_buf: &wgpu::Buffer,
    atlas_view: &wgpu::TextureView,
    atlas_sampler: &wgpu::Sampler,
    array_view: &wgpu::TextureView,
    array_sampler: &wgpu::Sampler,
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
    // The atlas loader caps the tile count at 256 (the packed vertex's 8-bit
    // tile-id field); this guards the table size against a cap drift.
    assert!(Tile::count() <= UV_RECTS_LEN);
    let mut uv_rects = [[0f32; 4]; UV_RECTS_LEN];
    for t in Tile::all() {
        uv_rects[t.index()] = tile_uv(t);
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

    // Terrain-only tile ARRAY (group 1 for the opaque/transparent block pipelines): one
    // layer per tile with REPEAT wrapping, so a greedy-meshed quad tiles its layer. The 2D
    // `atlas_bgl`/`atlas_bind` above stay for the model/break/particle/mob passes.
    let array_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("atlas array bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2Array,
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
    let atlas_array_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("atlas array bg"),
        layout: &array_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(array_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(array_sampler),
            },
        ],
    });
    let array_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("array pipe layout"),
        bind_group_layouts: &[&uniform_bgl, &array_bgl],
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

    let opaque_targets = color_target(
        format,
        Some(wgpu::BlendState::REPLACE),
        wgpu::ColorWrites::ALL,
    );
    let transparent_targets = color_target(
        format,
        Some(wgpu::BlendState::ALPHA_BLENDING),
        wgpu::ColorWrites::ALL,
    );

    let cull_back = wgpu::PrimitiveState {
        cull_mode: Some(wgpu::Face::Back),
        ..Default::default()
    };
    let opaque_pipe = world_pipeline(
        device,
        "opaque pipe",
        &array_layout,
        &shader,
        "vs_main",
        "fs_opaque",
        std::slice::from_ref(&vbuf_layout),
        &opaque_targets,
        cull_back,
        Some(DepthPreset::WriteLess),
        sample_count,
    );
    // Back-face cull water SIDE faces: otherwise a side face (e.g. an exposed
    // step over shallower water) shows its back as a dark sheet from the
    // water side, "in front of" the water that is actually there. The TOP
    // face is emitted in BOTH windings by the mesher, so the surface stays
    // visible from underneath (looking up while submerged) even with culling.
    // Depth `Less`, NO write so the water doesn't occlude geometry behind it.
    let transparent_pipe = world_pipeline(
        device,
        "transparent pipe",
        &array_layout,
        &shader,
        "vs_main",
        "fs_transparent",
        std::slice::from_ref(&vbuf_layout),
        &transparent_targets,
        cull_back,
        Some(DepthPreset::ReadLess),
        sample_count,
    );

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
    let sky_targets = color_target(
        format,
        Some(wgpu::BlendState::REPLACE),
        wgpu::ColorWrites::ALL,
    );
    let sky_pipe = world_pipeline(
        device,
        "sky pipe",
        &sky_layout,
        &sky_shader,
        "vs_sky",
        "fs_sky",
        &[],
        &sky_targets,
        wgpu::PrimitiveState::default(),
        None,
        sample_count,
    );

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
    let outline_targets = color_target(
        format,
        Some(wgpu::BlendState::REPLACE),
        wgpu::ColorWrites::ALL,
    );
    // Depth-test against terrain so edges behind blocks are hidden, but don't
    // write depth. The box is inflated slightly outward (see `outline_vertices`)
    // so visible front edges win the LessEqual test.
    let outline_pipe = world_pipeline(
        device,
        "outline pipe",
        &outline_layout,
        &outline_shader,
        "vs_outline",
        "fs_outline",
        &[outline_vbuf_layout],
        &outline_targets,
        wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::LineList,
            ..Default::default()
        },
        Some(DepthPreset::ReadLessEqual),
        sample_count,
    );
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
    // write_mask = COLOR (not ALL): the invert blend must leave the alpha channel
    // untouched.
    let crosshair_targets = color_target(format, Some(invert_blend), wgpu::ColorWrites::COLOR);
    let crosshair_pipe = world_pipeline(
        device,
        "crosshair pipe",
        &crosshair_layout,
        &crosshair_shader,
        "vs_crosshair",
        "fs_crosshair",
        &[crosshair_vbuf_layout],
        &crosshair_targets,
        wgpu::PrimitiveState::default(),
        None,
        sample_count,
    );
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
    let model3d_targets = color_target(
        format,
        Some(wgpu::BlendState::ALPHA_BLENDING),
        wgpu::ColorWrites::ALL,
    );
    // The two model3d pipelines share shader/layout/blend/cull and differ ONLY in
    // the depth attachment: `model3d_pipe` is depthless (the iso icons draw in the
    // depthless UI pass); `model3d_hand_pipe` adds depth Less + write so the
    // first-person held block self-sorts against the hand pass's cleared depth
    // buffer (a single pipeline cannot serve both passes).
    let model3d_pipe = world_pipeline(
        device,
        "model3d pipe",
        &model3d_layout,
        &model3d_shader,
        "vs_model",
        "fs_model",
        std::slice::from_ref(&model3d_vbuf_layout),
        &model3d_targets,
        cull_back,
        None,
        sample_count,
    );
    let model3d_hand_pipe = world_pipeline(
        device,
        "model3d hand pipe",
        &model3d_layout,
        &model3d_shader,
        "vs_model",
        "fs_model",
        std::slice::from_ref(&model3d_vbuf_layout),
        &model3d_targets,
        cull_back,
        Some(DepthPreset::WriteLess),
        sample_count,
    );
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
    // Vertex: pos (f32x3 @0) + uv (f32x2 @12) + shade (f32 @20) + tint (f32x3 @24)
    // = 36 bytes (matches `ItemVertex`).
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
        // tint (foliage-green for fern / short grass, white otherwise)
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x3,
            offset: 24,
            shader_location: 3,
        },
    ];
    let item3d_vbuf_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<super::item_model::ItemVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &item3d_vbuf_attrs,
    };
    let item3d_targets = color_target(
        format,
        Some(wgpu::BlendState::ALPHA_BLENDING),
        wgpu::ColorWrites::ALL,
    );
    // Double-sided (cull None): the back face + inward walls must never cull.
    // Depth-test + write against the hand pass's own (cleared) depth buffer so the
    // extruded mesh self-sorts: front, stepped side walls and back no longer
    // overdraw each other in submission order. The hand pass clears depth, so this
    // stays isolated from the world (the item still draws over terrain).
    let item3d_pipe = world_pipeline(
        device,
        "item3d pipe",
        &item3d_layout,
        &item3d_shader,
        "vs_item",
        "fs_item",
        std::slice::from_ref(&item3d_vbuf_layout),
        &item3d_targets,
        wgpu::PrimitiveState::default(),
        Some(DepthPreset::WriteLess),
        sample_count,
    );
    let item3d_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("item3d vbuf"),
        size: MAX_ITEM3D_VERTICES * std::mem::size_of::<super::item_model::ItemVertex>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    // --- mob pipeline (in-world animated entity models). ---
    // Reuses the BLOCK pipeline layout (`layout` = [uniform_bgl, atlas_bgl]): group0
    // is the world `view_proj` uniform (the shader reads only view_proj; the uv_rects
    // binding in the layout is simply unused), group1 is an atlas-shaped texture+
    // sampler — bound by the renderer to the ENTITY texture, not the block atlas.
    // Same explicit-UV `ItemVertex` layout as item3d (the model carries arbitrary
    // sub-rect UVs). REPLACE blend + cutout (opaque creature), depth test + WRITE,
    // double-sided (cull off) so flat mob sub-cubes show from both sides.
    let mob_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("mob shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/mob.wgsl").into()),
    });
    let mob_pipe = world_pipeline(
        device,
        "mob pipe",
        &layout,
        &mob_shader,
        "vs_mob",
        "fs_mob",
        std::slice::from_ref(&item3d_vbuf_layout),
        &opaque_targets,
        wgpu::PrimitiveState::default(),
        Some(DepthPreset::WriteLess),
        sample_count,
    );
    // The mob pipeline is shared across species; each species' own vbuf/ibuf +
    // bind group + DynamicDraw are built in the renderer by iterating `mob::MOB_DEFS`
    // (each species has a distinct texture, so geometry can't share one buffer).

    // --- Break-overlay pipeline (the destroy crack). ---
    // Reuses the block `uniform_bgl` (group0: view_proj + uv_rects) + `atlas_bgl`
    // (group1) so it binds the renderer's existing `uniform_bind` / `atlas_bind`
    // unchanged. Same 28-byte vertex as the block pipe. MULTIPLY-blended; depth
    // LessEqual / no-write; the cube is built coincident with the block faces and a
    // small polygon offset (BREAK_DEPTH_BIAS) wins the depth tie on the surface, so
    // the crack reads cleanly with no inflation and no z-fighting.
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
    let break_targets = color_target(format, Some(multiply_blend), wgpu::ColorWrites::ALL);
    // group0 = block uniform layout (Uniforms + uv_rects); group1 = atlas. Same
    // layout object as the opaque/transparent pipes (`layout`). Depth `LessEqual`,
    // NO write, with the BREAK_DEPTH_BIAS polygon offset (DepthPreset::
    // ReadLessEqualBiased): the crack cube is COINCIDENT with the block faces, but
    // the chunk mesher flips each face's triangulation diagonal per-AO (see
    // `should_flip` in mesh::face) while this cube always splits 0->2. Two
    // triangulations of the same plane interpolate depth a ULP apart per pixel,
    // which speckle-fights under a plain LessEqual; the small offset toward the
    // camera makes the crack win that tie everywhere, with no geometric inflation
    // to misalign the decal at glancing angles.
    let break_pipe = world_pipeline(
        device,
        "break overlay pipe",
        &layout,
        &break_shader,
        "vs_break",
        "fs_break",
        std::slice::from_ref(&break_vbuf_layout),
        &break_targets,
        cull_back,
        Some(DepthPreset::ReadLessEqualBiased),
        sample_count,
    );
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

    // Chest model dynamic buffers (drawn by the opaque pipeline, like item entities;
    // separate so a chest wall and the dropped-item budget don't fight).
    let chest_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("chest vbuf"),
        size: MAX_CHEST_VERTICES * std::mem::size_of::<Vertex>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let chest_ibuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("chest ibuf"),
        size: MAX_CHEST_INDICES * 4,
        usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let door_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("door vbuf"),
        size: MAX_DOOR_VERTICES * std::mem::size_of::<Vertex>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let door_ibuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("door ibuf"),
        size: MAX_DOOR_INDICES * 4,
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
    // Opaque cubes (cutout discard handles transparency) — no blend. Cubes carry
    // their own per-face winding; disabling cull is robust (and the cutout discard
    // means we never rely on backface rejection for the look). Depth Less + write.
    let particle_targets = color_target(format, None, wgpu::ColorWrites::ALL);
    let particle_pipe = world_pipeline(
        device,
        "particle pipe",
        &layout,
        &particle_shader,
        "vs_particle",
        "fs_particle",
        std::slice::from_ref(&particle_vbuf_layout),
        &particle_targets,
        wgpu::PrimitiveState::default(),
        Some(DepthPreset::WriteLess),
        sample_count,
    );
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
    // UI quads are CPU-emitted CCW but disabling cull is robust against either
    // winding; no depth (last pass). Alpha blend.
    let ui_targets = color_target(
        format,
        Some(wgpu::BlendState::ALPHA_BLENDING),
        wgpu::ColorWrites::ALL,
    );
    let ui_pipe = world_pipeline(
        device,
        "ui pipe",
        &ui_layout,
        &ui_shader,
        "vs_ui",
        "fs_ui",
        std::slice::from_ref(&ui_vbuf_layout),
        &ui_targets,
        wgpu::PrimitiveState::default(),
        None,
        sample_count,
    );
    let ui_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ui vbuf"),
        size: MAX_UI_VERTICES * std::mem::size_of::<super::ui::UiVertex>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    // --- model-icon pipeline (bbmodel-block icon-atlas cells). ---
    // Pass-through `ItemVertex` (positions already in clip space, the MVP baked in by
    // `build_block_model_icon`) sampling the MODEL atlas at group(0). Depth test +
    // WRITE: the double-sided model self-sorts by depth (the faces are also emitted
    // far→near as a tiebreak). The same `item3d`/`mob` ItemVertex layout (pos f32x3 @0,
    // uv f32x2 @12, shade f32 @20, tint f32x3 @24) feeds it, so the model-atlas
    // validation test covers it. Used only to bake the model cells of the icon atlas.
    let model_icon_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("model icon shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/model_icon.wgsl").into()),
    });
    let model_icon_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("model icon layout"),
        bind_group_layouts: &[&atlas_bgl],
        push_constant_ranges: &[],
    });
    let model_icon_vbuf_attrs = [
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
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x3,
            offset: 24,
            shader_location: 3,
        },
    ];
    let model_icon_vbuf_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<super::item_model::ItemVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &model_icon_vbuf_attrs,
    };
    let model_icon_targets = color_target(
        format,
        Some(wgpu::BlendState::ALPHA_BLENDING),
        wgpu::ColorWrites::ALL,
    );
    // Depth test + WRITE against the model-icon pass's OWN cleared depth buffer so the
    // (double-sided) model self-sorts — the panels/drawers can't be ordered by a painter
    // sort alone, exactly like the in-world block, which also leans on depth.
    let model_icon_pipe = world_pipeline(
        device,
        "model icon pipe",
        &model_icon_layout,
        &model_icon_shader,
        "vs_model_icon",
        "fs_model_icon",
        std::slice::from_ref(&model_icon_vbuf_layout),
        &model_icon_targets,
        wgpu::PrimitiveState::default(),
        Some(DepthPreset::WriteLess),
        sample_count,
    );

    PipelineResources {
        atlas_array_bind,
        uniform_bind,
        atlas_bind,
        atlas_bgl,
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
        model3d_mvp_bgl,
        uv_rects_buf,
        model3d_vbuf,
        model3d_ibuf,
        item3d_pipe,
        item3d_mvp_bind,
        item3d_vbuf,
        mob_pipe,
        break_pipe,
        break_vbuf,
        break_ibuf,
        item_entity_vbuf,
        item_entity_ibuf,
        chest_vbuf,
        chest_ibuf,
        door_vbuf,
        door_ibuf,
        particle_pipe,
        particle_vbuf,
        particle_ibuf,
        ui_pipe,
        ui_vbuf,
        model_icon_pipe,
    }
}

#[cfg(test)]
mod gpu_validation {
    use super::*;
    use crate::render::renderer::instance_descriptor;

    /// Headless validation that the REAL pipeline factory produces internally
    /// consistent pipelines: WGSL parses + passes naga validation, each pass's
    /// vertex attribute formats/locations match its shader's `VsIn`, and the
    /// bind-group layouts match the shaders' declared bindings. This calls the
    /// production `create_pipeline_resources` under a validation error scope and
    /// asserts nothing was reported — so it can never drift from the runtime
    /// pipelines the way a hand-copied descriptor would. Skips cleanly on
    /// machines/CI with no GPU adapter (the interactive demo is where final
    /// visual confirmation happens).
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

        // Fabricate the minimal external resources `create_pipeline_resources`
        // binds: a 1x1 Rgba8UnormSrgb texture view for both the block atlas and
        // the gui atlas (matches the real `Float { filterable: true }` / D2 BGLs),
        // a filtering sampler, and a uniform buffer sized to `Uniforms`. The
        // factory never samples or reads these in this test — it only builds bind
        // groups + pipelines — so 1x1 placeholders are sufficient to validate.
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("test atlas"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let atlas_view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        // A 1-layer D2Array view for the terrain pipeline's tile-array bind (matches the
        // real `D2Array` BGL). A D2 texture with one layer views fine as D2Array.
        let array_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("test atlas array"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let array_view = array_tex.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("test sampler"),
            ..Default::default()
        });
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("test uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        device.push_error_scope(wgpu::ErrorFilter::Validation);

        // Build EVERY real pipeline through the production factory. Any
        // shader/layout/vertex-attribute/blend/depth mismatch surfaces as a
        // captured validation error below.
        let _resources = create_pipeline_resources(
            &device,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            1,
            &uniform_buf,
            &atlas_view,
            &sampler,
            &array_view,
            &sampler,
        );

        let err = pollster::block_on(device.pop_error_scope());
        assert!(err.is_none(), "real-pipeline validation error: {err:?}");
        // Confirm the assumption baked into the packing: tile ids fit in 8 bits
        // (also enforced by the atlas loader at composition time).
        assert!(Tile::count() <= 256);
        // Stride sanity: the compressed block vertex is exactly 28 bytes.
        assert_eq!(std::mem::size_of::<Vertex>(), 28);
        // item3d vertex stride must match its declared attribute layout
        // (pos f32x3 @0, uv f32x2 @12, shade f32 @20, tint f32x3 @24 = 36 bytes).
        assert_eq!(
            std::mem::size_of::<crate::render::item_model::ItemVertex>(),
            36
        );
    }
}
