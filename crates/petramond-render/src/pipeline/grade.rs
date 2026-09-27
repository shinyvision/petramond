use super::builders::{
    color_target, pipeline_layout, shader_module, single_pipeline, texture_sampler_bind_entries,
    texture_sampler_layout_entries, uniform_entry,
};

#[cfg(test)]
mod tests;

pub(super) fn create_grade_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
) -> (wgpu::RenderPipeline, wgpu::BindGroupLayout) {
    let shader = shader_module(device, "grade shader", super::GRADE_SHADER);
    let mut entries = texture_sampler_layout_entries(0, wgpu::TextureViewDimension::D2).to_vec();
    entries.push(uniform_entry(2, wgpu::ShaderStages::FRAGMENT, 16));
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("grade bgl"),
        entries: &entries,
    });
    let layout = pipeline_layout(device, "grade layout", &[&bgl]);
    let grade_targets = color_target(format, None, wgpu::ColorWrites::ALL);
    let pipe = single_pipeline(
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
    );
    (pipe, bgl)
}

pub(crate) fn create_grade_bind(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
    scene_view: &wgpu::TextureView,
    post_process_buf: &wgpu::Buffer,
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
        resource: post_process_buf.as_entire_binding(),
    });
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("grade bind"),
        layout: bgl,
        entries: &entries,
    })
}
