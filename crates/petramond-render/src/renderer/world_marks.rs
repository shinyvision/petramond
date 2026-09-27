//! The world-marks pass ([`crate::world_marks`]): drawn onto the WINDOW after
//! the frame's capture point, over the frame and under the window's UI, so a
//! capture never holds a mark.
//!
//! Marks hide behind the world by the world's depth, but the frame's depth
//! buffer is the hand's by then (the hand pass clears it). So when any mark
//! can be hidden, the frame graph's `WorldMarksDepth` node keeps the world's
//! eye depth at the frame's size just before the hand pass, and the mark
//! fragments compare against that.

use std::sync::OnceLock;

use super::*;
use crate::world_marks::{bake, MarkBatch, MarkTex, MarkVertex, WorldMarks};

const SHADER: &str = include_str!("../../shaders/world_marks.wgsl");
const DEPTH_SHADER: &str = include_str!("../../shaders/world_marks_depth.wgsl");
const EYE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;
const PARAMS_BYTES: u64 = 32;
const DEPTH_SIZE_OFFSET: u64 = 16;

#[derive(Default)]
pub(super) struct WorldMarksPass {
    gpu: Option<MarksGpu>,
    verts: Vec<MarkVertex>,
    batches: Vec<MarkBatch>,
    batch_images: Vec<Option<usize>>,
    vbuf: Option<wgpu::Buffer>,
    images: super::client_overlay::ClientImageBinds,
    tests_depth: bool,
}

struct MarksGpu {
    device: wgpu::Device,
    pipe: wgpu::RenderPipeline,
    marks_bgl: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    keepers: [OnceLock<DepthKeeper>; 2],
    eye: Option<EyeDepth>,
}

struct DepthKeeper {
    pipe: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
}

struct EyeDepth {
    size: (u32, u32),
    view: wgpu::TextureView,
    bind: wgpu::BindGroup,
}

impl Renderer {
    pub fn set_world_marks(&mut self, marks: &WorldMarks) {
        let Renderer {
            device,
            queue,
            config,
            ui,
            view,
            binds,
            world_marks: pass,
            ..
        } = self;
        bake(
            marks,
            view.render_origin,
            &mut pass.verts,
            &mut pass.batches,
        );
        pass.batch_images.clear();
        pass.tests_depth = marks.tests_depth();
        pass.images
            .retain_keys(marks.items.iter().filter_map(|item| match item {
                crate::world_marks::WorldMark::Image { image, .. } => Some(image.key.as_str()),
                _ => None,
            }));
        if pass.batches.is_empty() {
            return;
        }
        let gpu = pass
            .gpu
            .get_or_insert_with(|| MarksGpu::new(device, config.format, &ui.texture_bgl));
        let size = (config.width, config.height);
        if gpu.eye.as_ref().is_none_or(|eye| eye.size != size) {
            gpu.eye = Some(gpu.eye_depth(size, &binds.uniform_buf));
        }
        let ui_gpu = super::doc_ui::UiGpu {
            device,
            queue,
            texture_bgl: &ui.texture_bgl,
        };
        for batch in &pass.batches {
            pass.batch_images.push(match batch.tex {
                MarkTex::Image(item) => match &marks.items[item] {
                    crate::world_marks::WorldMark::Image { image, .. } => {
                        Some(pass.images.ensure(&ui_gpu, image))
                    }
                    _ => None,
                },
                MarkTex::Font | MarkTex::Theme(_) => {
                    ui_gpu.theme(&mut ui.theme);
                    None
                }
                MarkTex::Solid => None,
            });
        }
        let buffer = pass.vbuf.get_or_insert_with(|| {
            super::dynamic_draw::new_buffer(device, wgpu::BufferUsages::VERTEX, "world marks")
        });
        super::dynamic_draw::upload(
            device,
            queue,
            buffer,
            &pass.verts,
            wgpu::BufferUsages::VERTEX,
            "world marks",
        );
    }

    pub(super) fn world_marks_keep_depth(&self) -> bool {
        let pass = &self.world_marks;
        pass.tests_depth
            && !pass.batches.is_empty()
            && pass
                .gpu
                .as_ref()
                .and_then(|gpu| gpu.eye.as_ref())
                .is_some_and(|eye| eye.size == self.screen_size())
    }

    pub(super) fn record_world_marks_depth(
        &self,
        rp: &mut wgpu::RenderPass<'_>,
        ctx: &super::passes::PassCtx<'_>,
    ) {
        let Some(gpu) = self.world_marks.gpu.as_ref() else {
            return;
        };
        let Some(eye) = gpu.eye.as_ref() else {
            return;
        };
        let keeper = gpu.keeper(ctx.samples > 1);
        self.queue.write_buffer(
            &gpu.params,
            DEPTH_SIZE_OFFSET,
            bytemuck::cast_slice(&[eye.size.0 as f32, eye.size.1 as f32, 0.0, 0.0]),
        );
        let bind = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("world marks depth"),
            layout: &keeper.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.targets.depth),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: ctx.binds.uniform_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: gpu.params.as_entire_binding(),
                },
            ],
        });
        rp.set_pipeline(&keeper.pipe);
        rp.set_bind_group(0, &bind, &[]);
        rp.draw(0..3, 0..1);
    }

    pub(super) fn encode_world_marks(
        &self,
        enc: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        rect: (f32, f32, f32, f32),
    ) {
        let pass = &self.world_marks;
        let (Some(gpu), Some(vbuf)) = (pass.gpu.as_ref(), pass.vbuf.as_ref()) else {
            return;
        };
        let Some(eye) = gpu.eye.as_ref() else {
            return;
        };
        if pass.batches.is_empty() || eye.size != self.screen_size() {
            return;
        }
        let (x, y, w, h) = rect;
        self.queue
            .write_buffer(&gpu.params, 0, bytemuck::cast_slice(&[x, y, w, h]));
        let mut rp =
            super::passes::overlay_pass(enc, target, "world marks", self.gpu_timer.as_ref());
        rp.set_viewport(x, y, w, h, 0.0, 1.0);
        rp.set_pipeline(&gpu.pipe);
        rp.set_bind_group(1, &eye.bind, &[]);
        rp.set_vertex_buffer(0, vbuf.slice(..));
        let theme = self.ui.theme.as_ref();
        for (batch, image) in pass.batches.iter().zip(&pass.batch_images) {
            let bind = match batch.tex {
                MarkTex::Solid => Some(&self.ui.icon_atlas.bind),
                MarkTex::Font => theme.map(|theme| &theme.font),
                MarkTex::Theme(page) => theme.and_then(|theme| theme.page(page)),
                MarkTex::Image(_) => image.and_then(|index| pass.images.bind(index)),
            };
            let Some(bind) = bind else {
                continue;
            };
            rp.set_bind_group(0, bind, &[]);
            rp.draw(batch.start..batch.start + batch.count, 0..1);
        }
    }
}

impl WorldMarksPass {
    pub(super) fn eye_view(&self) -> Option<&wgpu::TextureView> {
        self.gpu.as_ref()?.eye.as_ref().map(|eye| &eye.view)
    }
}

impl MarksGpu {
    fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        art_bgl: &wgpu::BindGroupLayout,
    ) -> Self {
        let marks_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("world marks"),
            entries: &[
                uniform_entry(0, wgpu::ShaderStages::VERTEX),
                uniform_entry(1, wgpu::ShaderStages::VERTEX_FRAGMENT),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let source = crate::pipeline::prelude::compose(SHADER)
            .unwrap_or_else(|e| panic!("world marks shader: {e}"));
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("world marks"),
            source: wgpu::ShaderSource::Wgsl(source),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("world marks"),
            bind_group_layouts: &[art_bgl, &marks_bgl],
            push_constant_ranges: &[],
        });
        let pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("world marks"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<MarkVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x3,
                        1 => Float32x3,
                        2 => Float32x2,
                        3 => Float32x2,
                        4 => Float32x4,
                        5 => Float32x2,
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("world marks params"),
            size: PARAMS_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            device: device.clone(),
            pipe,
            marks_bgl,
            params,
            keepers: [OnceLock::new(), OnceLock::new()],
            eye: None,
        }
    }

    fn eye_depth(&self, size: (u32, u32), uniforms: &wgpu::Buffer) -> EyeDepth {
        let texture = crate::gpu_mem::create_texture(
            &self.device,
            &wgpu::TextureDescriptor {
                label: Some("world marks eye depth"),
                size: wgpu::Extent3d {
                    width: size.0.max(1),
                    height: size.1.max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: EYE_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("world marks"),
            layout: &self.marks_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
            ],
        });
        EyeDepth { size, view, bind }
    }

    fn keeper(&self, multisampled: bool) -> &DepthKeeper {
        self.keepers[usize::from(multisampled)].get_or_init(|| {
            let device = &self.device;
            let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("world marks depth"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Depth,
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled,
                        },
                        count: None,
                    },
                    uniform_entry(1, wgpu::ShaderStages::FRAGMENT),
                    uniform_entry(2, wgpu::ShaderStages::FRAGMENT),
                ],
            });
            let source = crate::pipeline::scene_depth_source(multisampled, 0) + DEPTH_SHADER;
            let source = crate::pipeline::prelude::compose(&source)
                .unwrap_or_else(|e| panic!("world marks depth shader: {e}"))
                .into_owned();
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("world marks depth"),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("world marks depth"),
                bind_group_layouts: &[&bgl],
                push_constant_ranges: &[],
            });
            let pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("world marks depth"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_full"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_depth"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: EYE_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            });
            DepthKeeper { pipe, bgl }
        })
    }
}

fn uniform_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

#[cfg(test)]
mod tests;
