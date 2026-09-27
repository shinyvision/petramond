use std::num::NonZeroU64;

const BREAK_DEPTH_BIAS: wgpu::DepthBiasState = wgpu::DepthBiasState {
    constant: -10,
    slope_scale: -1.0,
    clamp: 0.0,
};

const _: () = assert!(
    BREAK_DEPTH_BIAS.constant < 0,
    "constant bias must be negative (toward camera)"
);
const _: () = assert!(
    BREAK_DEPTH_BIAS.slope_scale < 0.0,
    "slope-scaled bias must be negative (toward camera)"
);

const CONTACT_DEPTH_BIAS: wgpu::DepthBiasState = wgpu::DepthBiasState {
    constant: -10,
    slope_scale: -1.0,
    clamp: 0.0,
};

const _: () = assert!(
    CONTACT_DEPTH_BIAS.constant < 0,
    "constant bias must be negative (toward camera)"
);
const _: () = assert!(
    CONTACT_DEPTH_BIAS.slope_scale < 0.0,
    "slope-scaled bias must be negative (toward camera)"
);

pub(super) const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

const COPLANAR_WIN_DEPTH_BIAS: wgpu::DepthBiasState = wgpu::DepthBiasState {
    constant: -10,
    slope_scale: -1.0,
    clamp: 0.0,
};

const _: () = assert!(
    COPLANAR_WIN_DEPTH_BIAS.constant < 0 && COPLANAR_WIN_DEPTH_BIAS.slope_scale < 0.0,
    "a bias that wins coplanar ties must be negative (toward camera)"
);

#[derive(Copy, Clone)]
pub(super) enum DepthPreset {
    WriteLess,
    WriteLessEqualCoplanarBiased,
    ReadLess,
    ReadLessEqualBiased,
    ReadLessEqualContactBiased,
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
            DepthPreset::WriteLessEqualCoplanarBiased => (
                true,
                wgpu::CompareFunction::LessEqual,
                COPLANAR_WIN_DEPTH_BIAS,
            ),
            DepthPreset::ReadLess => (
                false,
                wgpu::CompareFunction::Less,
                wgpu::DepthBiasState::default(),
            ),
            DepthPreset::ReadLessEqualBiased => {
                (false, wgpu::CompareFunction::LessEqual, BREAK_DEPTH_BIAS)
            }
            DepthPreset::ReadLessEqualContactBiased => {
                (false, wgpu::CompareFunction::LessEqual, CONTACT_DEPTH_BIAS)
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

pub(super) fn color_target(
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

pub(super) fn shader_module(
    device: &wgpu::Device,
    label: &str,
    wgsl: impl Into<std::borrow::Cow<'static, str>>,
) -> wgpu::ShaderModule {
    let wgsl = wgsl.into();
    let composed = match super::prelude::compose(&wgsl) {
        Ok(std::borrow::Cow::Owned(text)) => Some(text),
        Ok(std::borrow::Cow::Borrowed(_)) => None,
        Err(e) => panic!("{label}: {e}"),
    };
    let source = composed.map_or(wgsl, std::borrow::Cow::Owned);
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source),
    })
}

pub(super) fn pipeline_layout(
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

pub(super) fn uniform_entry(
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

pub(super) fn buffer_bind_group(
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

pub(crate) fn texture_sampler_layout_entries(
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

pub(crate) fn texture_sampler_bind_entries<'a>(
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

pub(super) fn texture_sampler_bgl(
    device: &wgpu::Device,
    label: &str,
    dim: wgpu::TextureViewDimension,
) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries: &texture_sampler_layout_entries(0, dim),
    })
}

pub(super) fn texture_sampler_bgl_bind(
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

#[allow(clippy::too_many_arguments)]
pub(super) fn world_pipeline(
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
    max_samples: u32,
) -> super::SampledPipeline {
    let spec = super::sampling::PipelineSpec {
        label: label.to_owned(),
        layout: layout.clone(),
        shader: shader.clone(),
        vs_entry: vs_entry.to_owned(),
        fs_entry: fs_entry.to_owned(),
        buffers: buffers
            .iter()
            .map(super::sampling::VertexLayout::from_wgpu)
            .collect(),
        targets: targets.to_vec(),
        primitive,
        depth: depth.map(DepthPreset::state),
    };
    super::SampledPipeline::new(device, spec, max_samples)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn single_pipeline(
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
) -> wgpu::RenderPipeline {
    render_pipeline(
        device,
        label,
        layout,
        shader,
        vs_entry,
        fs_entry,
        buffers,
        targets,
        primitive,
        depth.map(DepthPreset::state),
        1,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_pipeline(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    vs_entry: &str,
    fs_entry: &str,
    buffers: &[wgpu::VertexBufferLayout],
    targets: &[Option<wgpu::ColorTargetState>],
    primitive: wgpu::PrimitiveState,
    depth_stencil: Option<wgpu::DepthStencilState>,
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
        depth_stencil,
        multisample: wgpu::MultisampleState {
            count: sample_count,
            ..Default::default()
        },
        multiview: None,
        cache: None,
    })
}

pub(super) fn cull_back() -> wgpu::PrimitiveState {
    wgpu::PrimitiveState {
        cull_mode: Some(wgpu::Face::Back),
        ..Default::default()
    }
}
