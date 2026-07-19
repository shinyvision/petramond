use crate::atlas::{tile_uv, Tile};
use crate::mesh::Vertex;

use wgpu::util::DeviceExt;

use super::uniforms::{Uniforms, UV_RECTS_LEN};
use super::{item_model, particles, resources, shader_pack, ui};

mod builders;
mod entity_models;
mod environment;
#[cfg(test)]
mod gpu_validation;
mod grade;
mod model3d;
mod overlays;
mod particle;
mod sky;
mod terrain;
mod ui_icons;

pub(super) use self::entity_models::{
    MAX_CHEST_INDICES, MAX_CHEST_VERTICES, MAX_DOOR_INDICES, MAX_DOOR_VERTICES,
    MAX_ITEM_ENTITY_INDICES, MAX_ITEM_ENTITY_VERTICES, MAX_MOB_INDICES, MAX_MOB_VERTICES,
};
pub(super) use self::environment::{
    create_env_comp_bind, create_env_down_bind, create_environment_bind, EnvPassResources,
    EnvScaler,
};
pub(super) use self::grade::create_grade_bind;
pub(super) use self::model3d::MAX_ITEM3D_VERTICES;
pub(super) use self::overlays::{MAX_BREAK_INDICES, MAX_BREAK_VERTICES};
pub(super) use self::ui_icons::MAX_UI_VERTICES;

use self::builders::{
    buffer_bind_group, pipeline_layout, shader_module, texture_sampler_bgl_bind, uniform_entry,
};
use self::entity_models::{
    create_entity_model_buffers, create_mob_pipeline, create_world_model_pipeline,
};
use self::environment::{create_env_scaler, create_environment_pipelines};
use self::grade::create_grade_pipeline;
use self::model3d::{create_item3d_pipeline, create_model3d_pipelines};
use self::overlays::{
    create_break_overlay_pipeline, create_contact_pipeline, create_crosshair_pipeline,
    create_selection_pipeline,
};
use self::particle::create_particle_pipeline;
use self::sky::create_sky_pipeline;
use self::terrain::create_terrain_pipelines;
use self::ui_icons::{create_model_icon_pipeline, create_ui_pipeline};

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
    /// Model→terrain contact-shadow pipeline: the packed columns'
    /// `ContactShadowVertex` streams, multiplicative, depth read-only with its
    /// own coplanar bias, drawn between the opaque and sky passes. Binds only
    /// the shared `uniform_bind` at group 0.
    pub contact_pipe: wgpu::RenderPipeline,
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
    let contact_pipe = create_contact_pipeline(device, format, sample_count, &shared.uniform_bgl);
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
        contact_pipe,
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

/// Bind groups / layouts shared across the per-pipeline constructors: the
/// uv-rect table, the frame-uniform group, the 2D atlas group, the block
/// pipeline layout, and the terrain tile-array group + layout.
struct SharedBindings {
    uv_rects_buf: wgpu::Buffer,
    /// The block group-0 LAYOUT (Uniforms + uv_rects), for pipelines that bind
    /// `uniform_bind` alone (the contact-shadow pass).
    uniform_bgl: wgpu::BindGroupLayout,
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
        uniform_bgl,
        uniform_bind,
        atlas_bgl,
        atlas_bind,
        layout,
        atlas_array_bind,
        array_layout,
    }
}
