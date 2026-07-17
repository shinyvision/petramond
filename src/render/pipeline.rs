use crate::atlas::{tile_uv, Tile};
use crate::mesh::Vertex;

use std::num::NonZeroU64;
use wgpu::util::DeviceExt;

use super::crosshair::MAX_CROSSHAIR_VERTICES;
use super::selection::MAX_OUTLINE_VERTICES;
use super::uniforms::{ShaderParams, Uniforms, UV_RECTS_LEN};

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
/// Sized per SPECIES (each gets its own buffers): a 16-cube sheep is ≤384 verts,
/// so this covers ~200 simultaneously-visible sheep — far above what worldgen
/// herds put in the streamed area. The bake also truncates to whole instances,
/// closest first, so exceeding the budget drops the farthest mobs instead of
/// blanking the species for the frame (see `dynamic_bake`).
pub(super) const MAX_MOB_VERTICES: u64 = 81920;
pub(super) const MAX_MOB_INDICES: u64 = 122880;
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
/// ~0); the `slope_scale` term covers glancing angles. A few ULP — far too
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
    /// Depth test `Less`, NO write. Transparent water and emitter particles:
    /// sort behind solid geometry without occluding the surfaces drawn after.
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

/// One labelled WGSL shader module (source from `include_str!`/`concat!`, or
/// the shader pack's owned string).
fn shader_module(
    device: &wgpu::Device,
    label: &str,
    wgsl: impl Into<std::borrow::Cow<'static, str>>,
) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(wgsl.into()),
    })
}

/// A pipeline layout over `bind_group_layouts`; no pass in this module uses
/// push constants.
fn pipeline_layout(
    device: &wgpu::Device,
    label: &str,
    bind_group_layouts: &[&wgpu::BindGroupLayout],
) -> wgpu::PipelineLayout {
    device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts,
        push_constant_ranges: &[],
    })
}

/// A whole-buffer uniform layout entry (no dynamic offset).
fn uniform_entry(
    binding: u32,
    visibility: wgpu::ShaderStages,
    min_size: u64,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: NonZeroU64::new(min_size),
        },
        count: None,
    }
}

/// The dynamic-offset per-draw MVP uniform entry: one mat4 (64 bytes) per
/// draw, selected by a 256-aligned dynamic offset (see
/// [`MODEL3D_MVP_SLOT_SIZE`]).
fn mvp_slot_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: true,
            min_binding_size: NonZeroU64::new(64),
        },
        count: None,
    }
}

/// The matching MVP resource: a 64-byte mat4 window over the slot buffer; the
/// per-draw dynamic offset selects the slot.
fn mvp_slot_binding(buf: &wgpu::Buffer) -> wgpu::BindingResource<'_> {
    wgpu::BindingResource::Buffer(wgpu::BufferBinding {
        buffer: buf,
        offset: 0,
        size: NonZeroU64::new(64),
    })
}

/// A bind group binding each buffer whole, at bindings `0..buffers.len()` in
/// order.
fn buffer_bind_group(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::BindGroupLayout,
    buffers: &[&wgpu::Buffer],
) -> wgpu::BindGroup {
    let entries: Vec<wgpu::BindGroupEntry> = buffers
        .iter()
        .enumerate()
        .map(|(i, buf)| wgpu::BindGroupEntry {
            binding: i as u32,
            resource: buf.as_entire_binding(),
        })
        .collect();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout,
        entries: &entries,
    })
}

/// The layout-entry pair of a fragment-sampled float texture (at `binding`)
/// plus its filtering sampler (at `binding + 1`).
fn texture_sampler_layout_entries(
    binding: u32,
    dim: wgpu::TextureViewDimension,
) -> [wgpu::BindGroupLayoutEntry; 2] {
    [
        wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: dim,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: binding + 1,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        },
    ]
}

/// The bind-group entry pair matching [`texture_sampler_layout_entries`].
fn texture_sampler_bind_entries<'a>(
    binding: u32,
    view: &'a wgpu::TextureView,
    sampler: &'a wgpu::Sampler,
) -> [wgpu::BindGroupEntry<'a>; 2] {
    [
        wgpu::BindGroupEntry {
            binding,
            resource: wgpu::BindingResource::TextureView(view),
        },
        wgpu::BindGroupEntry {
            binding: binding + 1,
            resource: wgpu::BindingResource::Sampler(sampler),
        },
    ]
}

/// A single texture+sampler bind-group layout (bindings 0/1).
fn texture_sampler_bgl(
    device: &wgpu::Device,
    label: &str,
    dim: wgpu::TextureViewDimension,
) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &texture_sampler_layout_entries(0, dim),
    })
}

/// Layout + bind group over one texture view + sampler; labels derive from
/// `label` (`"<label> bgl"` / `"<label> bg"`).
fn texture_sampler_bgl_bind(
    device: &wgpu::Device,
    label: &str,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    dim: wgpu::TextureViewDimension,
) -> (wgpu::BindGroupLayout, wgpu::BindGroup) {
    let bgl = texture_sampler_bgl(device, &format!("{label} bgl"), dim);
    let bg_label = format!("{label} bg");
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(&bg_label),
        layout: &bgl,
        entries: &texture_sampler_bind_entries(0, view, sampler),
    });
    (bgl, bind)
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
/// copies (120 verts per cube), so this is sized 5× the old single-copy budget
/// to still cover ~170 simultaneously-visible dropped items (more when they're
/// single, unstacked drops) without the bake overflowing and dropping every
/// item entity that frame. Also sizes the separate extruded-sprite item stream
/// (an extruded flower is a few hundred `ItemVertex` per layer, so that stream
/// covers dozens of visible sprite drops before its bake bails for a frame).
pub(super) const MAX_ITEM_ENTITY_VERTICES: u64 = 20480;
/// Max indices in the item-entity dynamic ibuf (up to 180 per cube for a
/// 5-layer stack), matching [`MAX_ITEM_ENTITY_VERTICES`].
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
    pub sky_texture_bind: wgpu::BindGroup,
    pub sky_shader_param_keys: Vec<String>,
    pub sky_light_param_key: Option<String>,
    /// Pack-supplied environment (volumetric) passes in pack load order,
    /// minus their depth-coupled group-0 binds (built by the renderer, which
    /// owns the depth view lifecycle).
    pub env_passes: Vec<EnvPassResources>,
    /// Half-res env scaler (downsample + composite around the env passes).
    pub env_scaler: EnvScaler,
    pub opaque_pipe: wgpu::RenderPipeline,
    pub translucent_pipe: wgpu::RenderPipeline,
    pub transparent_pipe: wgpu::RenderPipeline,
    /// Full-screen colour-grade pass: reads the offscreen scene texture, writes
    /// the swapchain (see `grade.wgsl`). The bind group over the scene view is
    /// built by [`create_grade_bind`] (and rebuilt on resize).
    pub grade_pipe: wgpu::RenderPipeline,
    pub grade_bgl: wgpu::BindGroupLayout,
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
    /// World-model pipeline: the chunk's bbmodel-block stream (`ModelVertex`,
    /// model atlas at group1). Same layout/blend/depth as `mob_pipe`, but its
    /// vertices carry (sky, block) light separately and the shader applies the
    /// sim's day/night sky scale at draw time, so placed models darken at
    /// night like terrain (their meshes don't rebake when the sun sets).
    pub world_model_pipe: wgpu::RenderPipeline,
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
    /// Cutout terrain-particle cube pipeline. Reuses the block `uniform_bind`
    /// + `atlas_bind`, depth-tests, and depth-writes.
    pub particle_pipe: wgpu::RenderPipeline,
    /// Translucent block-emitter particle pipeline: solid-color cube particles, alpha
    /// blended, depth-tested without writes, and back-face culled so transparency never
    /// exposes all six cube faces at once.
    pub emitter_particle_pipe: wgpu::RenderPipeline,
    /// Reusable dynamic vbuf for cutout particle cubes (rewritten in place per frame).
    pub particle_vbuf: wgpu::Buffer,
    /// Reusable dynamic vbuf for translucent block-emitter particles.
    pub emitter_particle_vbuf: wgpu::Buffer,
    /// Static ibuf for particle cubes, uploaded once.
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
    queue: &wgpu::Queue,
    format: wgpu::TextureFormat,
    sample_count: u32,
    uniform_buf: &wgpu::Buffer,
    shader_params_buf: &wgpu::Buffer,
    atlas_view: &wgpu::TextureView,
    atlas_sampler: &wgpu::Sampler,
    array_view: &wgpu::TextureView,
    array_sampler: &wgpu::Sampler,
) -> PipelineResources {
    let shader = shader_module(
        device,
        "block shader",
        concat!(
            include_str!("../shaders/cel.wgsl"),
            include_str!("../shaders/atmosphere.wgsl"),
            include_str!("../shaders/block.wgsl")
        ),
    );
    let crosshair_shader = shader_module(
        device,
        "crosshair shader",
        include_str!("../shaders/crosshair.wgsl"),
    );

    let shared = create_shared_bindings(
        device,
        uniform_buf,
        atlas_view,
        atlas_sampler,
        array_view,
        array_sampler,
    );

    // 24-byte packed vertex: pos (f32x3) + tint (unorm8x4, linear RGB) +
    // packed (u32) + packed2 (u32). Pipelines whose shaders ignore `packed2`
    // (break overlay) share the layout; an attribute the shader doesn't consume
    // is valid.
    let vbuf_attrs = [
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x3,
            offset: 0,
            shader_location: 0,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Unorm8x4,
            offset: 12,
            shader_location: 1,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Uint32,
            offset: 16,
            shader_location: 2,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Uint32,
            offset: 20,
            shader_location: 3,
        },
    ];
    let vbuf_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<Vertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &vbuf_attrs,
    };

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

    let (opaque_pipe, translucent_pipe, transparent_pipe) = create_terrain_pipelines(
        device,
        format,
        sample_count,
        &shader,
        &shared.array_layout,
        &vbuf_layout,
    );
    let sky = create_sky_pipeline(
        device,
        queue,
        format,
        sample_count,
        uniform_buf,
        shader_params_buf,
    );
    let env_passes = create_environment_pipelines(device, queue, format, sample_count);
    let env_scaler = create_env_scaler(device, format, sample_count);
    let (outline_pipe, outline_bind, outline_vbuf) =
        create_selection_pipeline(device, format, sample_count, uniform_buf);
    let (crosshair_pipe, crosshair_vbuf) =
        create_crosshair_pipeline(device, format, sample_count, &crosshair_shader);
    let model3d = create_model3d_pipelines(
        device,
        format,
        sample_count,
        uniform_buf,
        &shared.uv_rects_buf,
        &shared.atlas_bgl,
        &vbuf_layout,
    );
    let (item3d_pipe, item3d_mvp_bind, item3d_vbuf) = create_item3d_pipeline(
        device,
        format,
        sample_count,
        &shared.atlas_bgl,
        &model3d.mvp_buf,
        &item3d_vbuf_layout,
    );
    let (mob_pipe, mob_shader) = create_mob_pipeline(
        device,
        format,
        sample_count,
        &shared.layout,
        &item3d_vbuf_layout,
    );
    let world_model_pipe =
        create_world_model_pipeline(device, format, sample_count, &shared.layout, &mob_shader);
    let (break_pipe, break_vbuf, break_ibuf) =
        create_break_overlay_pipeline(device, format, sample_count, &shared.layout, &vbuf_layout);
    let entity_bufs = create_entity_model_buffers(device);
    let particles = create_particle_pipeline(device, format, sample_count, &shared.layout);
    let (ui_pipe, ui_vbuf) = create_ui_pipeline(device, format, sample_count);
    let model_icon_pipe =
        create_model_icon_pipeline(device, format, sample_count, &shared.atlas_bgl);
    let (grade_pipe, grade_bgl) = create_grade_pipeline(device, format, sample_count);

    PipelineResources {
        atlas_array_bind: shared.atlas_array_bind,
        uniform_bind: shared.uniform_bind,
        atlas_bind: shared.atlas_bind,
        atlas_bgl: shared.atlas_bgl,
        sky_pipe: sky.pipe,
        sky_bind: sky.bind,
        sky_texture_bind: sky.texture_bind,
        sky_shader_param_keys: sky.shader_param_keys,
        sky_light_param_key: sky.light_param_key,
        env_passes,
        env_scaler,
        opaque_pipe,
        translucent_pipe,
        transparent_pipe,
        grade_pipe,
        grade_bgl,
        outline_pipe,
        outline_bind,
        outline_vbuf,
        crosshair_pipe,
        crosshair_vbuf,
        model3d_pipe: model3d.pipe,
        model3d_hand_pipe: model3d.hand_pipe,
        model3d_mvp_buf: model3d.mvp_buf,
        model3d_mvp_bind: model3d.mvp_bind,
        model3d_mvp_bgl: model3d.mvp_bgl,
        uv_rects_buf: shared.uv_rects_buf,
        model3d_vbuf: model3d.vbuf,
        model3d_ibuf: model3d.ibuf,
        item3d_pipe,
        item3d_mvp_bind,
        item3d_vbuf,
        mob_pipe,
        world_model_pipe,
        break_pipe,
        break_vbuf,
        break_ibuf,
        item_entity_vbuf: entity_bufs.item_entity_vbuf,
        item_entity_ibuf: entity_bufs.item_entity_ibuf,
        chest_vbuf: entity_bufs.chest_vbuf,
        chest_ibuf: entity_bufs.chest_ibuf,
        door_vbuf: entity_bufs.door_vbuf,
        door_ibuf: entity_bufs.door_ibuf,
        particle_pipe: particles.pipe,
        emitter_particle_pipe: particles.emitter_pipe,
        particle_vbuf: particles.vbuf,
        emitter_particle_vbuf: particles.emitter_vbuf,
        particle_ibuf: particles.ibuf,
        ui_pipe,
        ui_vbuf,
        model_icon_pipe,
    }
}

/// Back-face-culled primitive state shared by the block-vertex passes
/// (opaque/transparent terrain, model3d, break overlay).
fn cull_back() -> wgpu::PrimitiveState {
    wgpu::PrimitiveState {
        cull_mode: Some(wgpu::Face::Back),
        ..Default::default()
    }
}

/// Bind groups / layouts shared across the per-pipeline constructors: the
/// uv-rect table, the frame-uniform group, the 2D atlas group, the block
/// pipeline layout, and the terrain tile-array group + layout.
struct SharedBindings {
    uv_rects_buf: wgpu::Buffer,
    uniform_bind: wgpu::BindGroup,
    atlas_bgl: wgpu::BindGroupLayout,
    atlas_bind: wgpu::BindGroup,
    /// The block pipeline layout ([uniform_bgl, atlas_bgl]), reused by the
    /// mob / world-model / break-overlay / particle passes.
    layout: wgpu::PipelineLayout,
    atlas_array_bind: wgpu::BindGroup,
    /// The terrain pipeline layout ([uniform_bgl, array_bgl]) for the
    /// opaque/transparent block passes.
    array_layout: wgpu::PipelineLayout,
}

fn create_shared_bindings(
    device: &wgpu::Device,
    uniform_buf: &wgpu::Buffer,
    atlas_view: &wgpu::TextureView,
    atlas_sampler: &wgpu::Sampler,
    array_view: &wgpu::TextureView,
    array_sampler: &wgpu::Sampler,
) -> SharedBindings {
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

    let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("uniform bgl"),
        entries: &[
            uniform_entry(
                0,
                wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                std::mem::size_of::<Uniforms>() as u64,
            ),
            uniform_entry(1, wgpu::ShaderStages::VERTEX, (UV_RECTS_LEN * 16) as u64),
        ],
    });
    let uniform_bind = buffer_bind_group(
        device,
        "uniform bg",
        &uniform_bgl,
        &[uniform_buf, &uv_rects_buf],
    );

    let (atlas_bgl, atlas_bind) = texture_sampler_bgl_bind(
        device,
        "atlas",
        atlas_view,
        atlas_sampler,
        wgpu::TextureViewDimension::D2,
    );
    let layout = pipeline_layout(device, "pipe layout", &[&uniform_bgl, &atlas_bgl]);

    // Terrain-only tile ARRAY (group 1 for the opaque/transparent block pipelines): one
    // layer per tile with REPEAT wrapping, so a greedy-meshed quad tiles its layer. The 2D
    // `atlas_bgl`/`atlas_bind` above stay for the model/break/particle/mob passes.
    let (array_bgl, atlas_array_bind) = texture_sampler_bgl_bind(
        device,
        "atlas array",
        array_view,
        array_sampler,
        wgpu::TextureViewDimension::D2Array,
    );
    let array_layout = pipeline_layout(device, "array pipe layout", &[&uniform_bgl, &array_bgl]);

    SharedBindings {
        uv_rects_buf,
        uniform_bind,
        atlas_bgl,
        atlas_bind,
        layout,
        atlas_array_bind,
        array_layout,
    }
}

/// The opaque + translucent-block (ice) + transparent (water) terrain
/// pipelines: the packed 32-byte block vertex over the tile-array pipeline
/// layout.
fn create_terrain_pipelines(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    sample_count: u32,
    shader: &wgpu::ShaderModule,
    array_layout: &wgpu::PipelineLayout,
    vbuf_layout: &wgpu::VertexBufferLayout,
) -> (
    wgpu::RenderPipeline,
    wgpu::RenderPipeline,
    wgpu::RenderPipeline,
) {
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
    let opaque_pipe = world_pipeline(
        device,
        "opaque pipe",
        array_layout,
        shader,
        "vs_main",
        "fs_opaque",
        std::slice::from_ref(vbuf_layout),
        &opaque_targets,
        cull_back(),
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
        array_layout,
        shader,
        "vs_main",
        "fs_transparent",
        std::slice::from_ref(vbuf_layout),
        &transparent_targets,
        cull_back(),
        Some(DepthPreset::ReadLess),
        sample_count,
    );
    // Translucent BLOCKS (ice) blend like water but WRITE depth and draw
    // before it: a 3D sheet of translucent cubes must resolve its own face
    // order through the depth buffer (within a section the buffer order is
    // arbitrary), and water behind/under the sheet then depth-fails instead
    // of double-blending over it. Shares `fs_transparent`, whose authored-
    // alpha split gives these tiles their own alpha (see block.wgsl).
    let translucent_pipe = world_pipeline(
        device,
        "translucent pipe",
        array_layout,
        shader,
        "vs_main",
        "fs_transparent",
        std::slice::from_ref(vbuf_layout),
        &transparent_targets,
        cull_back(),
        Some(DepthPreset::WriteLess),
        sample_count,
    );
    (opaque_pipe, translucent_pipe, transparent_pipe)
}

/// Load a pack shader row's declared texture paths into the four fixed
/// slots, blank-filling missing/undecodable slots, and bind them at
/// `slot*2`/`slot*2+1`. Shared by the sky and environment hooks.
fn create_shader_texture_bind(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture_bgl: &wgpu::BindGroupLayout,
    kind: &str,
    paths: &[String],
) -> wgpu::BindGroup {
    let mut slots = Vec::with_capacity(super::shader_pack::SKY_TEXTURE_SLOTS);
    for slot in 0..super::shader_pack::SKY_TEXTURE_SLOTS {
        let loaded = paths.get(slot).and_then(|rel| {
            let Some((bytes, path)) = crate::assets::read_bytes(rel) else {
                log::warn!("{kind} texture slot {slot} asset '{rel}' not found; using blank slot");
                return None;
            };
            match super::resources::create_sky_texture(device, queue, &bytes) {
                Some(texture) => {
                    log::info!("{kind} texture slot {slot} loaded from {}", path.display());
                    Some(texture)
                }
                None => {
                    log::warn!(
                        "{kind} texture slot {slot} asset '{}' is not a decodable PNG; using blank slot",
                        path.display()
                    );
                    None
                }
            }
        });
        slots.push(loaded.unwrap_or_else(|| {
            super::resources::create_solid_rgba_texture(
                device,
                queue,
                [0, 0, 0, 0],
                "blank shader texture",
            )
        }));
    }
    let mut entries = Vec::with_capacity(super::shader_pack::SKY_TEXTURE_SLOTS * 2);
    for (slot, (_, view, sampler)) in slots.iter().enumerate() {
        entries.extend(texture_sampler_bind_entries((slot * 2) as u32, view, sampler));
    }
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("pack shader texture bg"),
        layout: texture_bgl,
        entries: &entries,
    })
}

/// The values the sky pass hands back to [`PipelineResources`].
struct SkyResources {
    pipe: wgpu::RenderPipeline,
    bind: wgpu::BindGroup,
    texture_bind: wgpu::BindGroup,
    shader_param_keys: Vec<String>,
    light_param_key: Option<String>,
}

/// Sky-background pipeline.
/// Uses a sky-specific group 0 (frame uniforms + mod shader params) and a
/// fixed sky-texture group 1. It does not use terrain atlas resources or the
/// block pipeline's uv-rect table.
fn create_sky_pipeline(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    format: wgpu::TextureFormat,
    sample_count: u32,
    uniform_buf: &wgpu::Buffer,
    shader_params_buf: &wgpu::Buffer,
) -> SkyResources {
    let sky_spec = super::shader_pack::active_sky_shader();
    let sky_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("sky bgl"),
        entries: &[
            uniform_entry(
                0,
                wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                std::mem::size_of::<Uniforms>() as u64,
            ),
            uniform_entry(
                1,
                wgpu::ShaderStages::FRAGMENT,
                std::mem::size_of::<ShaderParams>() as u64,
            ),
        ],
    });
    let sky_bind = buffer_bind_group(
        device,
        "sky bg",
        &sky_bgl,
        &[uniform_buf, shader_params_buf],
    );
    let mut sky_texture_bgl_entries = Vec::with_capacity(super::shader_pack::SKY_TEXTURE_SLOTS * 2);
    for slot in 0..super::shader_pack::SKY_TEXTURE_SLOTS {
        sky_texture_bgl_entries.extend(texture_sampler_layout_entries(
            (slot * 2) as u32,
            wgpu::TextureViewDimension::D2,
        ));
    }
    let sky_texture_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("sky texture bgl"),
        entries: &sky_texture_bgl_entries,
    });
    let sky_texture_paths: &[String] = sky_spec
        .as_ref()
        .map_or(&[], |spec| spec.textures.as_slice());
    let sky_texture_bind = create_shader_texture_bind(
        device,
        queue,
        &sky_texture_bgl,
        "sky",
        sky_texture_paths,
    );
    let sky_layout = pipeline_layout(device, "sky layout", &[&sky_bgl, &sky_texture_bgl]);
    let sky_targets = color_target(
        format,
        Some(wgpu::BlendState::REPLACE),
        wgpu::ColorWrites::ALL,
    );
    let builtin_sky_shader = shader_module(
        device,
        "built-in sky shader",
        concat!(
            include_str!("../shaders/cel.wgsl"),
            include_str!("../shaders/atmosphere.wgsl"),
            include_str!("../shaders/sky.wgsl")
        ),
    );
    let sky_pipe_for = |shader: &wgpu::ShaderModule| {
        world_pipeline(
            device,
            "sky pipe",
            &sky_layout,
            shader,
            "vs_sky",
            "fs_sky",
            &[],
            &sky_targets,
            wgpu::PrimitiveState::default(),
            // The sky draws AFTER opaque terrain at exactly the far plane
            // (vs_sky emits z = 1.0), so LessEqual shades only the pixels no
            // terrain covered — the expensive sky fs skips the overdrawn ~80–90%.
            Some(DepthPreset::ReadLessEqual),
            sample_count,
        )
    };
    let sky_pipe = if let Some(spec) = sky_spec.as_ref() {
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let custom = shader_module(device, "pack sky shader", spec.source.clone());
        let pipe = sky_pipe_for(&custom);
        match pollster::block_on(device.pop_error_scope()) {
            Some(err) => {
                log::warn!(
                    "ignoring pack sky shader {}: validation failed: {err}",
                    spec.path.display()
                );
                sky_pipe_for(&builtin_sky_shader)
            }
            None => {
                log::info!("using pack sky shader {}", spec.path.display());
                pipe
            }
        }
    } else {
        sky_pipe_for(&builtin_sky_shader)
    };
    let sky_shader_param_keys = sky_spec
        .as_ref()
        .map_or_else(Vec::new, |spec| spec.params.clone());
    let sky_light_param_key = sky_spec
        .as_ref()
        .and_then(|spec| spec.sky_light_param.clone());

    SkyResources {
        pipe: sky_pipe,
        bind: sky_bind,
        texture_bind: sky_texture_bind,
        shader_param_keys: sky_shader_param_keys,
        light_param_key: sky_light_param_key,
    }
}

/// One pack-supplied environment (volumetric) pass, minus its depth-coupled
/// group-0 bind: the frame depth view is recreated on resize, so the bind is
/// built by [`create_environment_bind`] at construction AND on every scene-
/// target rebuild.
pub(super) struct EnvPassResources {
    pub pipe: wgpu::RenderPipeline,
    pub bgl: wgpu::BindGroupLayout,
    /// This pass's own 16-slot params buffer, filled per frame from its
    /// declared param keys (each pass has an independent key list).
    pub params_buf: wgpu::Buffer,
    pub texture_bind: wgpu::BindGroup,
    pub param_keys: Vec<String>,
}

/// The group-0 bind of an environment pass: frame uniforms, the pass's own
/// shader params, and the frame DEPTH as a sampled texture (the pass attaches
/// no depth, so sampling it is legal — volumetrics occlude themselves against
/// scene depth per fragment).
pub(super) fn create_environment_bind(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
    uniform_buf: &wgpu::Buffer,
    params_buf: &wgpu::Buffer,
    depth_view: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("environment bg"),
        layout: bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: params_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(depth_view),
            },
        ],
    })
}

/// Half-res environment scaler: pack environment passes render into a
/// half-res offscreen target against a downsampled depth, and a depth-aware
/// composite lifts the result back to full res (see `env_downsample.wgsl` /
/// `env_composite.wgsl` and the passes.rs environment block). Volumetrics
/// are soft, so half-res costs ~a quarter of the fragment work for a
/// near-identical image; the depth-aware upsample keeps silhouette edges
/// crisp.
pub(super) struct EnvScaler {
    pub down_pipe: wgpu::RenderPipeline,
    pub down_bgl: wgpu::BindGroupLayout,
    pub comp_pipe: wgpu::RenderPipeline,
    pub comp_bgl: wgpu::BindGroupLayout,
    pub samp: wgpu::Sampler,
}

fn depth_texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Depth,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn create_env_scaler(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    sample_count: u32,
) -> EnvScaler {
    let down_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("env downsample bgl"),
        entries: &[depth_texture_entry(0)],
    });
    let down_module = shader_module(
        device,
        "env downsample",
        include_str!("../shaders/env_downsample.wgsl"),
    );
    let down_layout = pipeline_layout(device, "env downsample layout", &[&down_bgl]);
    let down_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("env downsample pipe"),
        layout: Some(&down_layout),
        vertex: wgpu::VertexState {
            module: &down_module,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &down_module,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[],
        }),
        primitive: Default::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Always,
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        multiview: None,
        cache: None,
    });

    let comp_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("env composite bgl"),
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
            depth_texture_entry(2),
            depth_texture_entry(3),
        ],
    });
    let comp_module = shader_module(
        device,
        "env composite",
        include_str!("../shaders/env_composite.wgsl"),
    );
    let comp_layout = pipeline_layout(device, "env composite layout", &[&comp_bgl]);
    let targets = color_target(
        format,
        Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
        wgpu::ColorWrites::ALL,
    );
    let comp_pipe = world_pipeline(
        device,
        "env composite pipe",
        &comp_layout,
        &comp_module,
        "vs_main",
        "fs_main",
        &[],
        &targets,
        wgpu::PrimitiveState::default(),
        None,
        sample_count,
    );
    let samp = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("env composite sampler"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    EnvScaler { down_pipe, down_bgl, comp_pipe, comp_bgl, samp }
}

/// The downsample bind: just the full-res depth to read.
pub(super) fn create_env_down_bind(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
    full_depth: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("env downsample bg"),
        layout: bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(full_depth),
        }],
    })
}

/// The composite bind: half-res env colour + its depth, and the full-res
/// depth to resolve edges against.
pub(super) fn create_env_comp_bind(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
    env_color: &wgpu::TextureView,
    samp: &wgpu::Sampler,
    half_depth: &wgpu::TextureView,
    full_depth: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("env composite bg"),
        layout: bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(env_color),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(samp),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(half_depth),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(full_depth),
            },
        ],
    })
}

/// Pack `environment` rows → composed full-screen volumetric pipelines, in
/// pack load order. Shader ABI: `vs_env` emits a fullscreen triangle,
/// `fs_env` returns PREMULTIPLIED rgba blended over the scene; group 0 =
/// Uniforms (b0) + ShaderParams (b1) + frame depth (b2, `texture_depth_2d`,
/// read with `textureLoad`); group 1 = the four fixed texture slots. A row
/// whose WGSL fails validation is SKIPPED with one warning — there is no
/// builtin fallback (a missing volumetric is safe; a missing sky is not).
fn create_environment_pipelines(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    format: wgpu::TextureFormat,
    sample_count: u32,
) -> Vec<EnvPassResources> {
    let specs = super::shader_pack::environment_shaders();
    if specs.is_empty() {
        return Vec::new();
    }
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("environment bgl"),
        entries: &[
            uniform_entry(
                0,
                wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                std::mem::size_of::<Uniforms>() as u64,
            ),
            uniform_entry(
                1,
                wgpu::ShaderStages::FRAGMENT,
                std::mem::size_of::<ShaderParams>() as u64,
            ),
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
        ],
    });
    let mut texture_bgl_entries = Vec::with_capacity(super::shader_pack::SKY_TEXTURE_SLOTS * 2);
    for slot in 0..super::shader_pack::SKY_TEXTURE_SLOTS {
        texture_bgl_entries.extend(texture_sampler_layout_entries(
            (slot * 2) as u32,
            wgpu::TextureViewDimension::D2,
        ));
    }
    let texture_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("environment texture bgl"),
        entries: &texture_bgl_entries,
    });
    let layout = pipeline_layout(device, "environment layout", &[&bgl, &texture_bgl]);
    // Premultiplied output: a volumetric emits (radiance * alpha, alpha) and
    // composes over the scene without double-multiplying its own coverage.
    let targets = color_target(
        format,
        Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
        wgpu::ColorWrites::ALL,
    );
    let mut passes = Vec::with_capacity(specs.len());
    for spec in specs {
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = shader_module(device, "pack environment shader", spec.source.clone());
        let pipe = world_pipeline(
            device,
            "environment pipe",
            &layout,
            &module,
            "vs_env",
            "fs_env",
            &[],
            &targets,
            wgpu::PrimitiveState::default(),
            None,
            sample_count,
        );
        if let Some(err) = pollster::block_on(device.pop_error_scope()) {
            log::warn!(
                "skipping pack environment shader {}: validation failed: {err}",
                spec.path.display()
            );
            continue;
        }
        log::info!("using pack environment shader {}", spec.path.display());
        let params_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("environment shader params"),
            size: std::mem::size_of::<ShaderParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let texture_bind = create_shader_texture_bind(
            device,
            queue,
            &texture_bgl,
            "environment",
            &spec.textures,
        );
        passes.push(EnvPassResources {
            pipe,
            bgl: bgl.clone(),
            params_buf,
            texture_bind,
            param_keys: spec.params,
        });
    }
    passes
}

/// Full-screen colour-grade pipeline (`grade.wgsl`): one texture binding (the
/// offscreen scene target, read with `textureLoad` — no sampler), no vertex
/// buffers, no depth. Draws AFTER the world's hand pass and BEFORE the
/// crosshair/UI passes so screen chrome stays ungraded.
fn create_grade_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    sample_count: u32,
) -> (wgpu::RenderPipeline, wgpu::BindGroupLayout) {
    let shader = shader_module(
        device,
        "grade shader",
        include_str!("../shaders/grade.wgsl"),
    );
    // Filterable texture (the grade pass bilinearly upscales when the scene
    // renders below swapchain resolution) + the mod-mood uniform vec4.
    let mut entries =
        texture_sampler_layout_entries(0, wgpu::TextureViewDimension::D2).to_vec();
    entries.push(uniform_entry(2, wgpu::ShaderStages::FRAGMENT, 16));
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("grade bgl"),
        entries: &entries,
    });
    let layout = pipeline_layout(device, "grade layout", &[&bgl]);
    let grade_targets = color_target(format, None, wgpu::ColorWrites::ALL);
    let pipe = world_pipeline(
        device,
        "grade pipeline",
        &layout,
        &shader,
        "vs_grade",
        "fs_grade",
        &[],
        &grade_targets,
        wgpu::PrimitiveState::default(),
        None,
        sample_count,
    );
    (pipe, bgl)
}

/// The grade pass's bind group over the current offscreen scene view. Rebuilt
/// whenever the scene texture is recreated (init + every resize).
pub(super) fn create_grade_bind(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
    scene_view: &wgpu::TextureView,
    mood_buf: &wgpu::Buffer,
) -> wgpu::BindGroup {
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("grade sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let mut entries = texture_sampler_bind_entries(0, scene_view, &sampler).to_vec();
    entries.push(wgpu::BindGroupEntry {
        binding: 2,
        resource: mood_buf.as_entire_binding(),
    });
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("grade bind"),
        layout: bgl,
        entries: &entries,
    })
}

/// Selection-outline pipeline.
/// Its own minimal bind-group layout (Uniforms at binding 0 only) so it
/// doesn't couple to the block pipelines' uv_rects layout. Reuses the same
/// uniform buffer for view_proj.
fn create_selection_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    sample_count: u32,
    uniform_buf: &wgpu::Buffer,
) -> (wgpu::RenderPipeline, wgpu::BindGroup, wgpu::Buffer) {
    let outline_shader = shader_module(
        device,
        "outline shader",
        include_str!("../shaders/outline.wgsl"),
    );
    let outline_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("outline bgl"),
        entries: &[uniform_entry(
            0,
            wgpu::ShaderStages::VERTEX,
            std::mem::size_of::<Uniforms>() as u64,
        )],
    });
    let outline_bind = buffer_bind_group(device, "outline bg", &outline_bgl, &[uniform_buf]);
    let outline_layout = pipeline_layout(device, "outline layout", &[&outline_bgl]);
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
    (outline_pipe, outline_bind, outline_vbuf)
}

/// Center crosshair pipeline.
/// The fragment shader outputs white and the color blend computes
/// `white * (1 - dst) + dst * 0`, which inverts the pixels under the
/// crosshair instead of drawing a fixed light/dark color.
fn create_crosshair_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    sample_count: u32,
    crosshair_shader: &wgpu::ShaderModule,
) -> (wgpu::RenderPipeline, wgpu::Buffer) {
    let crosshair_layout = pipeline_layout(device, "crosshair layout", &[]);
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
        crosshair_shader,
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
    (crosshair_pipe, crosshair_vbuf)
}

/// The values the model3d pass hands back to [`PipelineResources`].
struct Model3dResources {
    pipe: wgpu::RenderPipeline,
    hand_pipe: wgpu::RenderPipeline,
    mvp_buf: wgpu::Buffer,
    mvp_bind: wgpu::BindGroup,
    mvp_bgl: wgpu::BindGroupLayout,
    vbuf: wgpu::Buffer,
    ibuf: wgpu::Buffer,
}

/// model3d pipeline (isometric slot icons + first-person held block).
/// group(0): a per-draw MVP mat4 via a DYNAMIC-OFFSET uniform (binding 0) plus
/// the shared uv_rects table (binding 1, same as the block pipeline). group(1):
/// the block atlas (reuse the atlas bgl shape). Full-bright, back-face culled,
/// alpha-blended so flat sprite items cut out. Built in TWO depth variants from
/// the SAME shader/layout: `model3d_pipe` (NO depth) for the depthless UI icon
/// pass, and `model3d_hand_pipe` (depth test + write) for the hand pass, which
/// now carries a cleared depth buffer so the held block self-sorts.
fn create_model3d_pipelines(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    sample_count: u32,
    uniform_buf: &wgpu::Buffer,
    uv_rects_buf: &wgpu::Buffer,
    atlas_bgl: &wgpu::BindGroupLayout,
    vbuf_layout: &wgpu::VertexBufferLayout,
) -> Model3dResources {
    let model3d_shader = shader_module(
        device,
        "model3d shader",
        include_str!("../shaders/model3d.wgsl"),
    );
    let model3d_mvp_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("model3d mvp bgl"),
        entries: &[
            mvp_slot_entry(0),
            uniform_entry(1, wgpu::ShaderStages::VERTEX, (UV_RECTS_LEN * 16) as u64),
            // The frame `Uniforms` buffer: model3d reads only fog_color.w (the
            // sim's sky scale) so the held block dims in step with terrain.
            uniform_entry(
                2,
                wgpu::ShaderStages::VERTEX,
                std::mem::size_of::<Uniforms>() as u64,
            ),
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
                resource: mvp_slot_binding(&model3d_mvp_buf),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: uv_rects_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: uniform_buf.as_entire_binding(),
            },
        ],
    });
    let model3d_layout = pipeline_layout(device, "model3d layout", &[&model3d_mvp_bgl, atlas_bgl]);
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
        std::slice::from_ref(vbuf_layout),
        &model3d_targets,
        cull_back(),
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
        std::slice::from_ref(vbuf_layout),
        &model3d_targets,
        cull_back(),
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

    Model3dResources {
        pipe: model3d_pipe,
        hand_pipe: model3d_hand_pipe,
        mvp_buf: model3d_mvp_buf,
        mvp_bind: model3d_mvp_bind,
        mvp_bgl: model3d_mvp_bgl,
        vbuf: model3d_vbuf,
        ibuf: model3d_ibuf,
    }
}

/// item3d pipeline (extruded first-person held item).
/// group(0) = a per-draw MVP via a DYNAMIC-OFFSET uniform (binding 0) over the
/// shared `model3d_mvp_buf` (reuses its 256-byte-slot pattern). group(1) = the
/// block atlas (reuse the atlas bgl). Explicit per-vertex (pos, uv, shade) so
/// the side walls can sample a single boundary texel's sub-UV (the model3d
/// packed-vertex shader can only SELECT whole-tile UV corners). Full-bright,
/// alpha-cutout, DOUBLE-SIDED (cull off so the back face + inner walls show),
/// NO depth (drawn over the world in the hand pass), alpha-blended.
fn create_item3d_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    sample_count: u32,
    atlas_bgl: &wgpu::BindGroupLayout,
    model3d_mvp_buf: &wgpu::Buffer,
    item3d_vbuf_layout: &wgpu::VertexBufferLayout,
) -> (wgpu::RenderPipeline, wgpu::BindGroup, wgpu::Buffer) {
    let item3d_shader = shader_module(
        device,
        "item3d shader",
        include_str!("../shaders/item3d.wgsl"),
    );
    let item3d_mvp_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("item3d mvp bgl"),
        entries: &[mvp_slot_entry(0)],
    });
    let item3d_mvp_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("item3d mvp bg"),
        layout: &item3d_mvp_bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: mvp_slot_binding(model3d_mvp_buf),
        }],
    });
    let item3d_layout = pipeline_layout(device, "item3d layout", &[&item3d_mvp_bgl, atlas_bgl]);
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
        std::slice::from_ref(item3d_vbuf_layout),
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
    (item3d_pipe, item3d_mvp_bind, item3d_vbuf)
}

/// mob pipeline (in-world animated entity models).
/// Reuses the BLOCK pipeline layout (`layout` = [uniform_bgl, atlas_bgl]): group0
/// is the world `view_proj` uniform (the shader reads only view_proj; the uv_rects
/// binding in the layout is simply unused), group1 is an atlas-shaped texture+
/// sampler — bound by the renderer to the ENTITY texture, not the block atlas.
/// Same explicit-UV `ItemVertex` layout as item3d (the model carries arbitrary
/// sub-rect UVs). REPLACE blend + cutout (opaque creature), depth test + WRITE,
/// double-sided (cull off) so flat mob sub-cubes show from both sides.
///
/// The mob pipeline is shared across species; each species' own vbuf/ibuf +
/// bind group + DynamicDraw are built in the renderer by iterating `mob::defs()`
/// (each species has a distinct texture, so geometry can't share one buffer).
/// Also returns the mob shader module, which the world-model pipeline shares.
fn create_mob_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    sample_count: u32,
    layout: &wgpu::PipelineLayout,
    item3d_vbuf_layout: &wgpu::VertexBufferLayout,
) -> (wgpu::RenderPipeline, wgpu::ShaderModule) {
    let opaque_targets = color_target(
        format,
        Some(wgpu::BlendState::REPLACE),
        wgpu::ColorWrites::ALL,
    );
    let mob_shader = shader_module(
        device,
        "mob shader",
        concat!(
            include_str!("../shaders/cel.wgsl"),
            include_str!("../shaders/atmosphere.wgsl"),
            include_str!("../shaders/mob.wgsl")
        ),
    );
    let mob_pipe = world_pipeline(
        device,
        "mob pipe",
        layout,
        &mob_shader,
        "vs_mob",
        "fs_mob",
        std::slice::from_ref(item3d_vbuf_layout),
        &opaque_targets,
        wgpu::PrimitiveState::default(),
        Some(DepthPreset::WriteLess),
        sample_count,
    );
    (mob_pipe, mob_shader)
}

/// world-model pipeline (chunk bbmodel-block stream).
/// `ModelVertex`: the ItemVertex attributes + a (sky, block) light pair at
/// @location(4), so `fs_world_model` can scale the sky term by the sim's
/// day/night state at draw time (chunk meshes don't rebake at sunset).
fn create_world_model_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    sample_count: u32,
    layout: &wgpu::PipelineLayout,
    mob_shader: &wgpu::ShaderModule,
) -> wgpu::RenderPipeline {
    let opaque_targets = color_target(
        format,
        Some(wgpu::BlendState::REPLACE),
        wgpu::ColorWrites::ALL,
    );
    let world_model_vbuf_attrs = [
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
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 36,
            shader_location: 4,
        },
    ];
    let world_model_vbuf_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<crate::mesh::ModelVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &world_model_vbuf_attrs,
    };
    let world_model_pipe = world_pipeline(
        device,
        "world model pipe",
        layout,
        mob_shader,
        "vs_world_model",
        "fs_world_model",
        std::slice::from_ref(&world_model_vbuf_layout),
        &opaque_targets,
        wgpu::PrimitiveState::default(),
        Some(DepthPreset::WriteLess),
        sample_count,
    );
    world_model_pipe
}

/// Break-overlay pipeline (the destroy crack).
/// Reuses the block `uniform_bgl` (group0: view_proj + uv_rects) + `atlas_bgl`
/// (group1) so it binds the renderer's existing `uniform_bind` / `atlas_bind`
/// unchanged. Same 32-byte vertex as the block pipe. MULTIPLY-blended; depth
/// LessEqual / no-write; the cube is built coincident with the block faces and a
/// small polygon offset (BREAK_DEPTH_BIAS) wins the depth tie on the surface, so
/// the crack reads cleanly with no inflation and no z-fighting.
fn create_break_overlay_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    sample_count: u32,
    layout: &wgpu::PipelineLayout,
    vbuf_layout: &wgpu::VertexBufferLayout,
) -> (wgpu::RenderPipeline, wgpu::Buffer, wgpu::Buffer) {
    let break_shader = shader_module(
        device,
        "break overlay shader",
        concat!(
            include_str!("../shaders/cel.wgsl"),
            include_str!("../shaders/atmosphere.wgsl"),
            include_str!("../shaders/break_overlay.wgsl")
        ),
    );
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
        layout,
        &break_shader,
        "vs_break",
        "fs_break",
        std::slice::from_ref(vbuf_layout),
        &break_targets,
        cull_back(),
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
    (break_pipe, break_vbuf, break_ibuf)
}

/// Dynamic vertex/index buffers for item-entity, chest, and door models (all
/// drawn by the opaque pipeline; separate budgets so one kind can't starve
/// another).
struct EntityModelBuffers {
    item_entity_vbuf: wgpu::Buffer,
    item_entity_ibuf: wgpu::Buffer,
    chest_vbuf: wgpu::Buffer,
    chest_ibuf: wgpu::Buffer,
    door_vbuf: wgpu::Buffer,
    door_ibuf: wgpu::Buffer,
}

fn create_entity_model_buffers(device: &wgpu::Device) -> EntityModelBuffers {
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

    EntityModelBuffers {
        item_entity_vbuf,
        item_entity_ibuf,
        chest_vbuf,
        chest_ibuf,
        door_vbuf,
        door_ibuf,
    }
}

struct ParticlePipelineResources {
    pipe: wgpu::RenderPipeline,
    emitter_pipe: wgpu::RenderPipeline,
    vbuf: wgpu::Buffer,
    emitter_vbuf: wgpu::Buffer,
    ibuf: wgpu::Buffer,
}

/// Particle pipelines (tiny 3D cubes). Mining/break particles use alpha cutout and
/// depth writes. Block-row emitter particles use solid vertex colors, alpha blending,
/// depth read-only, and back-face culling.
fn create_particle_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    sample_count: u32,
    layout: &wgpu::PipelineLayout,
) -> ParticlePipelineResources {
    let particle_shader = shader_module(
        device,
        "particle shader",
        concat!(
            include_str!("../shaders/cel.wgsl"),
            include_str!("../shaders/atmosphere.wgsl"),
            include_str!("../shaders/particles.wgsl")
        ),
    );
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
        layout,
        &particle_shader,
        "vs_particle",
        "fs_particle",
        std::slice::from_ref(&particle_vbuf_layout),
        &particle_targets,
        wgpu::PrimitiveState::default(),
        Some(DepthPreset::WriteLess),
        sample_count,
    );
    let emitter_targets = color_target(
        format,
        Some(wgpu::BlendState::ALPHA_BLENDING),
        wgpu::ColorWrites::ALL,
    );
    let emitter_pipe = world_pipeline(
        device,
        "emitter particle pipe",
        layout,
        &particle_shader,
        "vs_particle",
        "fs_particle_transparent",
        std::slice::from_ref(&particle_vbuf_layout),
        &emitter_targets,
        cull_back(),
        Some(DepthPreset::ReadLess),
        sample_count,
    );
    // Deliberately tiny: DynamicVertexDraw::bake grows the buffer on demand up
    // to MAX_PARTICLE_VERTICES (the two worst-case buffers were ~8 MB of VRAM,
    // parked mostly empty).
    let particle_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("particle vbuf"),
        size: 4096,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let emitter_particle_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("emitter particle vbuf"),
        size: 4096,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    // Static quad indices, uploaded once (only the vbuf is rewritten per frame).
    let particle_ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("particle ibuf"),
        contents: bytemuck::cast_slice(&super::particles::particle_indices()),
        usage: wgpu::BufferUsages::INDEX,
    });
    ParticlePipelineResources {
        pipe: particle_pipe,
        emitter_pipe,
        vbuf: particle_vbuf,
        emitter_vbuf: emitter_particle_vbuf,
        ibuf: particle_ibuf,
    }
}

/// UI pipeline (2D HUD / inventory).
/// group(0) is the SEPARATE gui sprite atlas (texture + sampler) — NOT the
/// block atlas. Vertices are NDC pos (vec2) + uv (vec2) + color (vec4); the
/// fragment shader outputs the vertex color for the solid sentinel (uv.x < 0)
/// and otherwise samples the gui atlas * color. Alpha-blended, NO depth, drawn
/// LAST so it sits over every world / hand / crosshair pass.
fn create_ui_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    sample_count: u32,
) -> (wgpu::RenderPipeline, wgpu::Buffer) {
    let ui_shader = shader_module(device, "ui shader", include_str!("../shaders/ui.wgsl"));
    let ui_bgl = texture_sampler_bgl(device, "ui bgl", wgpu::TextureViewDimension::D2);
    let ui_layout = pipeline_layout(device, "ui layout", &[&ui_bgl]);
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
    (ui_pipe, ui_vbuf)
}

/// model-icon pipeline (bbmodel-block icon-atlas cells).
/// Pass-through `ItemVertex` (positions already in clip space, the MVP baked in by
/// `build_block_model_icon`) sampling the MODEL atlas at group(0). Depth test +
/// WRITE: the double-sided model self-sorts by depth (the faces are also emitted
/// far→near as a tiebreak). The same `item3d`/`mob` ItemVertex layout (pos f32x3 @0,
/// uv f32x2 @12, shade f32 @20, tint f32x3 @24) feeds it, so the model-atlas
/// validation test covers it. Used only to bake the model cells of the icon atlas.
fn create_model_icon_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    sample_count: u32,
    atlas_bgl: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let model_icon_shader = shader_module(
        device,
        "model icon shader",
        include_str!("../shaders/model_icon.wgsl"),
    );
    let model_icon_layout = pipeline_layout(device, "model icon layout", &[atlas_bgl]);
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
    model_icon_pipe
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
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
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
        let shader_params_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("test shader params"),
            size: std::mem::size_of::<ShaderParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        device.push_error_scope(wgpu::ErrorFilter::Validation);

        // Build EVERY real pipeline through the production factory. Any
        // shader/layout/vertex-attribute/blend/depth mismatch surfaces as a
        // captured validation error below.
        let _resources = create_pipeline_resources(
            &device,
            &queue,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            1,
            &uniform_buf,
            &shader_params_buf,
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
        // Stride sanity: the compressed block vertex is exactly 24 bytes
        // (unorm8 tint + two packed u32 words).
        assert_eq!(std::mem::size_of::<Vertex>(), 24);
        // item3d vertex stride must match its declared attribute layout
        // (pos f32x3 @0, uv f32x2 @12, shade f32 @20, tint f32x3 @24 = 36 bytes).
        assert_eq!(
            std::mem::size_of::<crate::render::item_model::ItemVertex>(),
            36
        );
    }
}
