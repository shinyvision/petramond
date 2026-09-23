use super::builders::{color_target, shader_module, world_pipeline, DepthPreset};

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
pub(super) fn create_mob_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    max_samples: u32,
    layout: &wgpu::PipelineLayout,
    item3d_vbuf_layout: &wgpu::VertexBufferLayout,
) -> (crate::pipeline::SampledPipeline, wgpu::ShaderModule) {
    let opaque_targets = color_target(
        format,
        Some(wgpu::BlendState::REPLACE),
        wgpu::ColorWrites::ALL,
    );
    let mob_shader = shader_module(
        device,
        "mob shader",
        [
            include_str!("../../shaders/cel.wgsl"),
            include_str!("../../shaders/atmosphere.wgsl"),
            &super::flipbook::model_declarations(),
            crate::selection_highlight::SHADER,
            include_str!("../../shaders/mob.wgsl"),
            // The break-crack decal over a model block: same module, so it
            // draws with the model's own vertex stage and texture sampling.
            include_str!("../../shaders/model_break.wgsl"),
        ]
        .concat(),
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
        max_samples,
    );
    (mob_pipe, mob_shader)
}

/// `ModelVertex`'s attributes: pos / uv / shade / packed light / packed tint.
/// Shared, because the break-crack decal MUST draw the model stream with the
/// same layout it was drawn with.
const WORLD_MODEL_ATTRS: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
    0 => Float32x3,
    1 => Float32x2,
    2 => Float32,
    3 => Uint32,
    4 => Uint32,
];

fn world_model_vbuf_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<petramond_mesh::ModelVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &WORLD_MODEL_ATTRS,
    }
}

/// world-model pipeline (chunk bbmodel-block stream).
/// `ModelVertex`: pos/uv/shade plus the (sky, block rgb) light as four PACKED
/// 6-bit levels at @location(3), so `fs_world_model` can scale the sky term by
/// the sim's day/night state at draw time (chunk meshes don't rebake at sunset)
/// and apply the block light's colour per channel, plus a packed per-vertex
/// TINT at @location(4) — the cell's `petramond:tint` on the cubes its row
/// declares tintable (a dyed part, a species colour, a heat glow). White on
/// every other vertex.
///
/// `blended` selects the alpha-BLEND variant (`fs_world_model_blend`) for the
/// chunk's semi-transparent model faces: same vertex layout and depth
/// test+write (the ice precedent — a 3D pocket of blended faces resolves its
/// own order through the depth buffer), drawn in the model-blend pass.
pub(super) fn create_world_model_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    max_samples: u32,
    layout: &wgpu::PipelineLayout,
    mob_shader: &wgpu::ShaderModule,
    blended: bool,
) -> crate::pipeline::SampledPipeline {
    let targets = color_target(
        format,
        Some(if blended {
            wgpu::BlendState::ALPHA_BLENDING
        } else {
            wgpu::BlendState::REPLACE
        }),
        wgpu::ColorWrites::ALL,
    );
    let world_model_vbuf_layout = world_model_vbuf_layout();
    world_pipeline(
        device,
        if blended {
            "world model blend pipe"
        } else {
            "world model pipe"
        },
        layout,
        mob_shader,
        "vs_world_model",
        if blended {
            "fs_world_model_blend"
        } else {
            "fs_world_model"
        },
        &[
            world_model_vbuf_layout,
            crate::resources::COLUMN_ORIGIN_LAYOUT,
        ],
        &targets,
        // Back-face CULLED: every solid-cube face bakes with its outward CCW
        // winding, and culling stops the far side of a cube ghosting through
        // the near face's cutout texels (the bright-line artifact on thin
        // cubes like the forge furnace's coals panel). The one kept face of a
        // zero-thickness plane bakes an explicit reversed duplicate, so decals
        // still show from both sides.
        wgpu::PrimitiveState {
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        },
        Some(DepthPreset::WriteLess),
        max_samples,
    )
}

/// model-break pipeline: the destroy crack over a bbmodel block, drawn as a
/// decal over the model's OWN triangles (see `model_break.wgsl`). Same shader
/// module, same vertex layout and same back-face culling as the world-model
/// pipeline, so it rasterizes exactly the fragments the model pass rasterized;
/// MULTIPLY blend and depth `LessEqual` / no write make it darken that surface.
/// group(2) carries the frame's crack masks + the block atlas.
pub(super) fn create_model_break_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    max_samples: u32,
    layout: &wgpu::PipelineLayout,
    mob_shader: &wgpu::ShaderModule,
) -> crate::pipeline::SampledPipeline {
    let targets = color_target(
        format,
        Some(super::overlays::MULTIPLY_BLEND),
        wgpu::ColorWrites::ALL,
    );
    world_pipeline(
        device,
        "model break pipe",
        layout,
        mob_shader,
        "vs_model_break",
        "fs_model_break",
        &[
            world_model_vbuf_layout(),
            crate::resources::COLUMN_ORIGIN_LAYOUT,
        ],
        &targets,
        wgpu::PrimitiveState {
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        },
        Some(DepthPreset::ReadLessEqualBiased),
        max_samples,
    )
}
