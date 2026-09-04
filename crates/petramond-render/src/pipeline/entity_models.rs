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
            include_str!("../../shaders/cel.wgsl"),
            include_str!("../../shaders/atmosphere.wgsl"),
            include_str!("../../shaders/mob.wgsl")
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
    sample_count: u32,
    layout: &wgpu::PipelineLayout,
    mob_shader: &wgpu::ShaderModule,
    blended: bool,
) -> wgpu::RenderPipeline {
    let targets = color_target(
        format,
        Some(if blended {
            wgpu::BlendState::ALPHA_BLENDING
        } else {
            wgpu::BlendState::REPLACE
        }),
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
            format: wgpu::VertexFormat::Uint32,
            offset: 24,
            shader_location: 3,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Uint32,
            offset: 28,
            shader_location: 4,
        },
    ];
    let world_model_vbuf_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<petramond_mesh::ModelVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &world_model_vbuf_attrs,
    };
    let world_model_pipe = world_pipeline(
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
        std::slice::from_ref(&world_model_vbuf_layout),
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
        sample_count,
    );
    world_model_pipe
}
