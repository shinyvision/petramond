use super::builders::{
    color_target, pipeline_layout, shader_module, texture_sampler_layout_entries, uniform_entry,
    world_pipeline, DEPTH_FORMAT,
};
use super::sky::create_shader_texture_bind;
use crate::render::uniforms::{ShaderParams, Uniforms};

/// One pack-supplied environment (volumetric) pass, minus its depth-coupled
/// group-0 bind: the frame depth view is recreated on resize, so the bind is
/// built by [`create_environment_bind`] at construction AND on every scene-
/// target rebuild.
pub(in crate::render) struct EnvPassResources {
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
pub(in crate::render) fn create_environment_bind(
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
pub(in crate::render) struct EnvScaler {
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

pub(super) fn create_env_scaler(
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
        include_str!("../../shaders/env_downsample.wgsl"),
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
        include_str!("../../shaders/env_composite.wgsl"),
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
pub(in crate::render) fn create_env_down_bind(
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
pub(in crate::render) fn create_env_comp_bind(
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
pub(super) fn create_environment_pipelines(
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
