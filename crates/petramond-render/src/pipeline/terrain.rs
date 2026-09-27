use super::builders::{color_target, cull_back, world_pipeline, DepthPreset};

pub(super) fn create_terrain_pipelines(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    max_samples: u32,
    shader: &wgpu::ShaderModule,
    array_layout: &wgpu::PipelineLayout,
    vbuf_layouts: &[wgpu::VertexBufferLayout],
) -> (
    crate::pipeline::SampledPipeline,
    crate::pipeline::SampledPipeline,
    crate::pipeline::SampledPipeline,
    crate::pipeline::SampledPipeline,
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
        "terrain opaque pipe",
        array_layout,
        shader,
        "vs_terrain",
        "fs_opaque",
        vbuf_layouts,
        &opaque_targets,
        cull_back(),
        Some(DepthPreset::WriteLess),
        max_samples,
    );
    // TRANSLUCENT fluid SIDE faces (an opaque fluid draws with the opaque
    // terrain above). Back-face culled: otherwise a side face (e.g. an exposed
    // step over shallower water) shows its back as a dark sheet from the fluid
    // side, "in front of" the fluid that is actually there. Depth `Less`, NO
    // write, so a see-through body never occludes the geometry behind it.
    let transparent_pipe = world_pipeline(
        device,
        "terrain transparent pipe",
        array_layout,
        shader,
        "vs_terrain",
        "fs_transparent",
        vbuf_layouts,
        &transparent_targets,
        cull_back(),
        Some(DepthPreset::ReadLess),
        max_samples,
    );
    // Translucent BLOCKS (ice) blend like water but WRITE depth and draw
    // before it: a 3D sheet of translucent cubes must resolve its own face
    // order through the depth buffer (within a section the buffer order is
    // arbitrary), and water behind/under the sheet then depth-fails instead
    // of double-blending over it. Shares `fs_transparent`, whose authored-
    // alpha split gives these tiles their own alpha (see block.wgsl).
    let translucent_pipe = world_pipeline(
        device,
        "terrain translucent pipe",
        array_layout,
        shader,
        "vs_terrain",
        "fs_transparent",
        vbuf_layouts,
        &transparent_targets,
        cull_back(),
        Some(DepthPreset::WriteLess),
        max_samples,
    );
    let transparent_two_sided_pipe = world_pipeline(
        device,
        "terrain transparent two-sided pipe",
        array_layout,
        shader,
        "vs_terrain",
        "fs_transparent",
        vbuf_layouts,
        &transparent_targets,
        wgpu::PrimitiveState::default(),
        Some(DepthPreset::ReadLess),
        max_samples,
    );
    (
        opaque_pipe,
        translucent_pipe,
        transparent_pipe,
        transparent_two_sided_pipe,
    )
}
