use super::builders::{
    color_target, pipeline_layout, shader_module, uniform_entry, world_pipeline, DepthPreset,
};

pub(crate) fn ghost(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    samples: u32,
    uniform: &wgpu::BindGroupLayout,
    atlas: &wgpu::BindGroupLayout,
    model: bool,
) -> super::SampledPipeline {
    let source = if model {
        [
            include_str!("../../shaders/cel.wgsl"),
            include_str!("../../shaders/atmosphere.wgsl"),
            &super::flipbook::model_declarations(),
            crate::selection_highlight::SHADER,
            include_str!("../../shaders/mob.wgsl"),
        ]
        .concat()
    } else {
        super::block_shader_source(&super::fluid_media::registered())
    };
    let shader = shader_module(device, "schematic ghost", source);
    let layout = pipeline_layout(device, "schematic ghost", &[uniform, atlas]);
    let blocks = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<petramond_mesh::Vertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &wgpu::vertex_attr_array![0=>Float32x3, 1=>Unorm8x4, 2=>Uint32, 3=>Uint32],
    };
    let models = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<petramond_mesh::ModelVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &wgpu::vertex_attr_array![0=>Float32x3, 1=>Float32x2, 2=>Float32, 3=>Uint32, 4=>Uint32],
    };
    let buffers = if model {
        vec![models, crate::resources::COLUMN_ORIGIN_LAYOUT]
    } else {
        vec![blocks]
    };
    world_pipeline(
        device,
        "schematic ghost",
        &layout,
        &shader,
        if model { "vs_world_model" } else { "vs_main" },
        if model {
            "fs_schematic_model"
        } else {
            "fs_schematic"
        },
        &buffers,
        &color_target(
            format,
            Some(wgpu::BlendState::ALPHA_BLENDING),
            wgpu::ColorWrites::ALL,
        ),
        super::builders::cull_back(),
        Some(DepthPreset::WriteLessEqualCoplanarBiased),
        samples,
    )
}

/// A flat-colour line or triangle overlay in world space.
pub(crate) fn flat(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    samples: u32,
    lines: bool,
) -> (super::SampledPipeline, wgpu::BindGroupLayout) {
    let shader = shader_module(
        device,
        "flat overlay",
        include_str!("../../shaders/flat_overlay.wgsl"),
    );
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("flat overlay"),
        entries: &[uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT, 80)],
    });
    let layout = pipeline_layout(device, "flat overlay", &[&bgl]);
    let pipeline = world_pipeline(
        device,
        "flat overlay",
        &layout,
        &shader,
        "vs_main",
        "fs_main",
        &[wgpu::VertexBufferLayout {
            array_stride: 12,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0=>Float32x3],
        }],
        &color_target(
            format,
            Some(wgpu::BlendState::ALPHA_BLENDING),
            wgpu::ColorWrites::ALL,
        ),
        wgpu::PrimitiveState {
            topology: if lines {
                wgpu::PrimitiveTopology::LineList
            } else {
                wgpu::PrimitiveTopology::TriangleList
            },
            cull_mode: None,
            ..Default::default()
        },
        Some(DepthPreset::ReadLessEqual),
        samples,
    );
    (pipeline, bgl)
}
