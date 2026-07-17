use super::builders::{color_target, cull_back, world_pipeline, DepthPreset};

/// The opaque + translucent-block (ice) + transparent (water) terrain
/// pipelines: the packed 32-byte block vertex over the tile-array pipeline
/// layout.
pub(super) fn create_terrain_pipelines(
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
