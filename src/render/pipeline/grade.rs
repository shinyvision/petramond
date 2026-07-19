use super::builders::{
    color_target, pipeline_layout, shader_module, texture_sampler_bind_entries,
    texture_sampler_layout_entries, uniform_entry, world_pipeline,
};

/// Full-screen colour-grade pipeline (`grade.wgsl`): one texture binding (the
/// offscreen scene target, read with `textureLoad` — no sampler), no vertex
/// buffers, no depth. Draws AFTER the world's hand pass and BEFORE the
/// crosshair/UI passes so screen chrome stays ungraded.
pub(super) fn create_grade_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    sample_count: u32,
) -> (wgpu::RenderPipeline, wgpu::BindGroupLayout) {
    let shader = shader_module(
        device,
        "grade shader",
        include_str!("../../shaders/grade.wgsl"),
    );
    // Filterable texture (the grade pass bilinearly upscales when the scene
    // renders below swapchain resolution) + the mod-mood uniform vec4.
    let mut entries = texture_sampler_layout_entries(0, wgpu::TextureViewDimension::D2).to_vec();
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
pub(in crate::render) fn create_grade_bind(
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
