use crate::atlas::{tile_uv, Tile, TILE_COUNT};
use crate::mesh::Vertex;

use std::num::NonZeroU64;
use wgpu::util::DeviceExt;

use super::crosshair::MAX_CROSSHAIR_VERTICES;
use super::selection::MAX_OUTLINE_VERTICES;
use super::uniforms::{Uniforms, UV_RECTS_LEN};

pub(super) struct PipelineResources {
    pub uniform_bind: wgpu::BindGroup,
    pub atlas_bind: wgpu::BindGroup,
    pub sky_pipe: wgpu::RenderPipeline,
    pub sky_bind: wgpu::BindGroup,
    pub opaque_pipe: wgpu::RenderPipeline,
    pub transparent_pipe: wgpu::RenderPipeline,
    pub outline_pipe: wgpu::RenderPipeline,
    pub outline_bind: wgpu::BindGroup,
    pub outline_vbuf: wgpu::Buffer,
    pub crosshair_pipe: wgpu::RenderPipeline,
    pub crosshair_vbuf: wgpu::Buffer,
}

pub(super) fn create_pipeline_resources(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    sample_count: u32,
    uniform_buf: &wgpu::Buffer,
    atlas_view: &wgpu::TextureView,
    atlas_sampler: &wgpu::Sampler,
) -> PipelineResources {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("block shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/block.wgsl").into()),
    });
    let sky_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("sky shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/sky.wgsl").into()),
    });
    let crosshair_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("crosshair shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/crosshair.wgsl").into()),
    });

    // uv-rect table: the EXACT `tile_uv()` bits per tile, indexed by `Tile as
    // usize`. The vertex shader only SELECTS corners from this (no arithmetic),
    // so reconstructed uvs are bit-identical to the old CPU-baked per-vertex uvs
    // on every backend (incl. WebGL2). Never updated after creation.
    const _: () = assert!(TILE_COUNT <= UV_RECTS_LEN);
    let mut uv_rects = [[0f32; 4]; UV_RECTS_LEN];
    for &t in Tile::ALL {
        uv_rects[t as usize] = tile_uv(t);
    }
    let uv_rects_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("uv_rects"),
        contents: bytemuck::cast_slice(&uv_rects[..]),
        usage: wgpu::BufferUsages::UNIFORM,
    });

    let uniform_bind_layout = wgpu::BindGroupLayoutDescriptor {
        label: Some("uniform bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(std::mem::size_of::<Uniforms>() as u64),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new((UV_RECTS_LEN * 16) as u64),
                },
                count: None,
            },
        ],
    };
    let uniform_bgl = device.create_bind_group_layout(&uniform_bind_layout);
    let uniform_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("uniform bg"),
        layout: &uniform_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: uv_rects_buf.as_entire_binding(),
            },
        ],
    });

    let atlas_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("atlas bgl"),
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
        ],
    });
    let atlas_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("atlas bg"),
        layout: &atlas_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(atlas_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(atlas_sampler),
            },
        ],
    });

    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("pipe layout"),
        bind_group_layouts: &[&uniform_bgl, &atlas_bgl],
        push_constant_ranges: &[],
    });

    // 28-byte packed vertex: pos (f32x3) + tint (f32x3) + packed (u32).
    let vbuf_attrs = [
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x3,
            offset: 0,
            shader_location: 0,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x3,
            offset: 12,
            shader_location: 1,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Uint32,
            offset: 24,
            shader_location: 2,
        },
    ];
    let vbuf_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<Vertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &vbuf_attrs,
    };

    let opaque_targets = vec![Some(wgpu::ColorTargetState {
        format,
        blend: Some(wgpu::BlendState::REPLACE),
        write_mask: wgpu::ColorWrites::ALL,
    })];
    let transparent_targets = vec![Some(wgpu::ColorTargetState {
        format,
        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
        write_mask: wgpu::ColorWrites::ALL,
    })];

    let opaque_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("opaque pipe"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[vbuf_layout.clone()],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_opaque"),
            compilation_options: Default::default(),
            targets: &opaque_targets,
        }),
        primitive: wgpu::PrimitiveState {
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: sample_count,
            ..Default::default()
        },
        multiview: None,
        cache: None,
    });
    let transparent_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("transparent pipe"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[vbuf_layout],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_transparent"),
            compilation_options: Default::default(),
            targets: &transparent_targets,
        }),
        // Double-sided: water faces must be visible from underneath (looking up at
        // the surface while submerged) as well as from above.
        primitive: wgpu::PrimitiveState {
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: sample_count,
            ..Default::default()
        },
        multiview: None,
        cache: None,
    });

    // --- Sky-background pipeline. ---
    // Uses its own Uniforms-only bind group because the sky shader does not need
    // atlas resources or the block pipeline's uv-rect table.
    let sky_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("sky bgl"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: NonZeroU64::new(std::mem::size_of::<Uniforms>() as u64),
            },
            count: None,
        }],
    });
    let sky_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("sky bg"),
        layout: &sky_bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform_buf.as_entire_binding(),
        }],
    });
    let sky_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("sky layout"),
        bind_group_layouts: &[&sky_bgl],
        push_constant_ranges: &[],
    });
    let sky_targets = [Some(wgpu::ColorTargetState {
        format,
        blend: Some(wgpu::BlendState::REPLACE),
        write_mask: wgpu::ColorWrites::ALL,
    })];
    let sky_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("sky pipe"),
        layout: Some(&sky_layout),
        vertex: wgpu::VertexState {
            module: &sky_shader,
            entry_point: Some("vs_sky"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &sky_shader,
            entry_point: Some("fs_sky"),
            compilation_options: Default::default(),
            targets: &sky_targets,
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState {
            count: sample_count,
            ..Default::default()
        },
        multiview: None,
        cache: None,
    });

    // --- Selection-outline pipeline. ---
    // Its own minimal bind-group layout (Uniforms at binding 0 only) so it
    // doesn't couple to the block pipelines' uv_rects layout. Reuses the same
    // uniform buffer for view_proj.
    let outline_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("outline shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/outline.wgsl").into()),
    });
    let outline_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("outline bgl"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: NonZeroU64::new(std::mem::size_of::<Uniforms>() as u64),
            },
            count: None,
        }],
    });
    let outline_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("outline bg"),
        layout: &outline_bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform_buf.as_entire_binding(),
        }],
    });
    let outline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("outline layout"),
        bind_group_layouts: &[&outline_bgl],
        push_constant_ranges: &[],
    });
    let outline_vbuf_layout = wgpu::VertexBufferLayout {
        array_stride: 12, // vec3<f32>
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x3,
            offset: 0,
            shader_location: 0,
        }],
    };
    let outline_targets = [Some(wgpu::ColorTargetState {
        format,
        blend: Some(wgpu::BlendState::REPLACE),
        write_mask: wgpu::ColorWrites::ALL,
    })];
    let outline_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("outline pipe"),
        layout: Some(&outline_layout),
        vertex: wgpu::VertexState {
            module: &outline_shader,
            entry_point: Some("vs_outline"),
            compilation_options: Default::default(),
            buffers: &[outline_vbuf_layout],
        },
        fragment: Some(wgpu::FragmentState {
            module: &outline_shader,
            entry_point: Some("fs_outline"),
            compilation_options: Default::default(),
            targets: &outline_targets,
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::LineList,
            ..Default::default()
        },
        // Depth-test against terrain so edges behind blocks are hidden, but
        // don't write depth. The box is inflated slightly outward (see
        // `outline_vertices`) so visible front edges win the LessEqual test.
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::LessEqual,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: sample_count,
            ..Default::default()
        },
        multiview: None,
        cache: None,
    });
    // Selection outline vertices x vec3<f32>.
    let outline_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("outline vbuf"),
        size: (MAX_OUTLINE_VERTICES * 12) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    // --- Center crosshair pipeline. ---
    // The fragment shader outputs white and the color blend computes
    // `white * (1 - dst) + dst * 0`, which inverts the pixels under the
    // crosshair instead of drawing a fixed light/dark color.
    let crosshair_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("crosshair layout"),
        bind_group_layouts: &[],
        push_constant_ranges: &[],
    });
    let crosshair_vbuf_layout = wgpu::VertexBufferLayout {
        array_stride: 8, // vec2<f32>
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 0,
            shader_location: 0,
        }],
    };
    let invert_blend = wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::OneMinusDst,
            dst_factor: wgpu::BlendFactor::Zero,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
    };
    let crosshair_targets = [Some(wgpu::ColorTargetState {
        format,
        blend: Some(invert_blend),
        write_mask: wgpu::ColorWrites::COLOR,
    })];
    let crosshair_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("crosshair pipe"),
        layout: Some(&crosshair_layout),
        vertex: wgpu::VertexState {
            module: &crosshair_shader,
            entry_point: Some("vs_crosshair"),
            compilation_options: Default::default(),
            buffers: &[crosshair_vbuf_layout],
        },
        fragment: Some(wgpu::FragmentState {
            module: &crosshair_shader,
            entry_point: Some("fs_crosshair"),
            compilation_options: Default::default(),
            targets: &crosshair_targets,
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState {
            count: sample_count,
            ..Default::default()
        },
        multiview: None,
        cache: None,
    });
    let crosshair_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("crosshair vbuf"),
        size: (MAX_CROSSHAIR_VERTICES * 8) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    PipelineResources {
        uniform_bind,
        atlas_bind,
        sky_pipe,
        sky_bind,
        opaque_pipe,
        transparent_pipe,
        outline_pipe,
        outline_bind,
        outline_vbuf,
        crosshair_pipe,
        crosshair_vbuf,
    }
}

#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod gpu_validation {
    use super::*;
    use crate::render::instance_descriptor;

    /// Headless validation that the packed-vertex pipeline is internally
    /// consistent: WGSL parses + passes naga validation, the vertex attribute
    /// formats/locations match the shader's `VsIn`, and the bind-group layouts
    /// match the shader's declared bindings (group0: Uniforms + uv_rects;
    /// group1: atlas texture + sampler). Any mismatch surfaces as a captured
    /// validation error. Skips cleanly on machines/CI with no GPU adapter
    /// (the interactive demo is where final visual confirmation happens).
    #[test]
    fn packed_vertex_pipeline_validates() {
        let instance = wgpu::Instance::new(&instance_descriptor());
        let adapter =
            match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::LowPower,
                compatible_surface: None,
                force_fallback_adapter: false,
            })) {
                Ok(a) => a,
                Err(_) => {
                    eprintln!("[skip] no wgpu adapter; pipeline validation not run");
                    return;
                }
            };
        let (device, _queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: None,
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default().using_alignment(adapter.limits()),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            }))
            .expect("device");

        device.push_error_scope(wgpu::ErrorFilter::Validation);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("block shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/block.wgsl").into()),
        });

        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(std::mem::size_of::<Uniforms>() as u64),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new((UV_RECTS_LEN * 16) as u64),
                    },
                    count: None,
                },
            ],
        });
        let atlas_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
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
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&uniform_bgl, &atlas_bgl],
            push_constant_ranges: &[],
        });

        let vbuf_attrs = [
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 0,
                shader_location: 0,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 12,
                shader_location: 1,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Uint32,
                offset: 24,
                shader_location: 2,
            },
        ];
        let vbuf_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &vbuf_attrs,
        };
        let targets = [Some(wgpu::ColorTargetState {
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            blend: Some(wgpu::BlendState::REPLACE),
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let _pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[vbuf_layout],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_opaque"),
                compilation_options: Default::default(),
                targets: &targets,
            }),
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // Also validate the outline pipeline + shader (LineList, group0 = a
        // minimal Uniforms-only bind group).
        let outline_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("outline shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/outline.wgsl").into()),
        });
        let outline_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(std::mem::size_of::<Uniforms>() as u64),
                },
                count: None,
            }],
        });
        let outline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&outline_bgl],
            push_constant_ranges: &[],
        });
        let outline_vbuf_layout = wgpu::VertexBufferLayout {
            array_stride: 12,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 0,
                shader_location: 0,
            }],
        };
        let _outline_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&outline_layout),
            vertex: wgpu::VertexState {
                module: &outline_shader,
                entry_point: Some("vs_outline"),
                compilation_options: Default::default(),
                buffers: &[outline_vbuf_layout],
            },
            fragment: Some(wgpu::FragmentState {
                module: &outline_shader,
                entry_point: Some("fs_outline"),
                compilation_options: Default::default(),
                targets: &targets,
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: false,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // Validate the fullscreen sky pipeline too. It uses the same Uniforms
        // layout but no vertex buffers, atlas resources, or depth attachment.
        let sky_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sky shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/sky.wgsl").into()),
        });
        let sky_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(std::mem::size_of::<Uniforms>() as u64),
                },
                count: None,
            }],
        });
        let sky_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&sky_bgl],
            push_constant_ranges: &[],
        });
        let _sky_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&sky_layout),
            vertex: wgpu::VertexState {
                module: &sky_shader,
                entry_point: Some("vs_sky"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &sky_shader,
                entry_point: Some("fs_sky"),
                compilation_options: Default::default(),
                targets: &targets,
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // Validate the crosshair pipeline, including the destination-color blend
        // used to invert the pixels under the crosshair.
        let crosshair_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("crosshair shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/crosshair.wgsl").into()),
        });
        let crosshair_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[],
            push_constant_ranges: &[],
        });
        let crosshair_vbuf_layout = wgpu::VertexBufferLayout {
            array_stride: 8,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x2,
                offset: 0,
                shader_location: 0,
            }],
        };
        let crosshair_targets = [Some(wgpu::ColorTargetState {
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            blend: Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::OneMinusDst,
                    dst_factor: wgpu::BlendFactor::Zero,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::Zero,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
            }),
            write_mask: wgpu::ColorWrites::COLOR,
        })];
        let _crosshair_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&crosshair_layout),
            vertex: wgpu::VertexState {
                module: &crosshair_shader,
                entry_point: Some("vs_crosshair"),
                compilation_options: Default::default(),
                buffers: &[crosshair_vbuf_layout],
            },
            fragment: Some(wgpu::FragmentState {
                module: &crosshair_shader,
                entry_point: Some("fs_crosshair"),
                compilation_options: Default::default(),
                targets: &crosshair_targets,
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let err = pollster::block_on(device.pop_error_scope());
        assert!(
            err.is_none(),
            "packed-vertex pipeline validation error: {err:?}"
        );
        // Confirm the assumption baked into the packing: tile ids fit in 8 bits.
        assert!(TILE_COUNT <= 256);
        // Stride sanity: the compressed vertex is exactly 28 bytes.
        assert_eq!(std::mem::size_of::<Vertex>(), 28);
    }
}
