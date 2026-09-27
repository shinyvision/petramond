//! Frames rendered at a size the window does not have (a frame-size claim, or
//! a document viewport the world presents in): one owned target, one
//! letterbox route.
//!
//! While sized frames are on, the renderer's FRAME geometry (`config` size,
//! the scene targets, the scene's UI viewport, the camera aspect a caller
//! derives from them) is theirs, and the window keeps its own swapchain size
//! and its own UI viewport. Each frame draws through the one `encode_frame`
//! into an owned [`FrameTarget`] of that size, shown on the window
//! letterboxed with the window's UI over it. A capture copies that target at
//! the frame's capture point (see `capture`).

use super::frame::{FrameOut, WindowOut};
use super::*;

const BLIT_SHADER: &str = r#"
@group(0) @binding(0) var frame_tex: texture_2d<f32>;
@group(0) @binding(1) var frame_samp: sampler;
struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};
@vertex
fn vs_blit(@builtin(vertex_index) i: u32) -> Out {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var out: Out;
    out.pos = vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    out.uv = uv;
    return out;
}
@fragment
fn fs_blit(in: Out) -> @location(0) vec4<f32> {
    return textureSample(frame_tex, frame_samp, in.uv);
}
"#;

/// An owned image a frame's scene lands in, which can be copied out and
/// shown on the window (scaled to fit).
pub(super) struct FrameTarget {
    pub(super) size: (u32, u32),
    pub(super) texture: wgpu::Texture,
    pub(super) view: wgpu::TextureView,
    blit_pipe: wgpu::RenderPipeline,
    blit_bind: wgpu::BindGroup,
}

impl FrameTarget {
    pub(super) fn new(
        device: &wgpu::Device,
        size: (u32, u32),
        format: wgpu::TextureFormat,
    ) -> Self {
        let texture = crate::gpu_mem::create_texture(
            device,
            &wgpu::TextureDescriptor {
                label: Some("frame target"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let (blit_pipe, blit_bind) = blit(device, &view, format);
        Self {
            size,
            texture,
            view,
            blit_pipe,
            blit_bind,
        }
    }

    /// Draw this image into `dst`, scaled to the `(x, y, w, h)` rect of it
    /// and the rest cleared black.
    pub(super) fn blit_into(
        &self,
        enc: &mut wgpu::CommandEncoder,
        dst: &wgpu::TextureView,
        rect: (f32, f32, f32, f32),
        label: &str,
    ) {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: dst,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        let (x, y, w, h) = rect;
        pass.set_viewport(x, y, w, h, 0.0, 1.0);
        pass.set_pipeline(&self.blit_pipe);
        pass.set_bind_group(0, &self.blit_bind, &[]);
        pass.draw(0..3, 0..1);
    }
}

pub(super) struct SizedFrames {
    pub(super) frame: FrameTarget,
    /// The window's swapchain size, kept apart from the frame size.
    surface_size: (u32, u32),
    /// Where on the window the frame is shown (window px `[x, y, w, h]`),
    /// scaled to fit inside; `None` = the whole window.
    destination: Option<[u32; 4]>,
}

impl SizedFrames {
    /// Where the frame lands on a `window`-sized image: letterboxed into
    /// the destination rect, or into the whole window.
    pub(super) fn placement(&self, window: (u32, u32)) -> (f32, f32, f32, f32) {
        match self.destination {
            Some([x, y, w, h]) => {
                let (dx, dy, dw, dh) = letterbox(self.frame.size, (w, h));
                (x as f32 + dx, y as f32 + dy, dw, dh)
            }
            None => letterbox(self.frame.size, window),
        }
    }
}

/// Where a `src`-sized image sits inside `dst`, scaled to fit with its aspect
/// kept: `(x, y, width, height)` in `dst` pixels.
pub(super) fn letterbox(src: (u32, u32), dst: (u32, u32)) -> (f32, f32, f32, f32) {
    let (sw, sh) = (src.0.max(1) as f32, src.1.max(1) as f32);
    let (dw, dh) = (dst.0.max(1) as f32, dst.1.max(1) as f32);
    let scale = (dw / sw).min(dh / sh);
    let (w, h) = ((sw * scale).max(1.0), (sh * scale).max(1.0));
    (((dw - w) * 0.5).floor(), ((dh - h) * 0.5).floor(), w, h)
}

impl Renderer {
    /// The device's own limits on one frame: its largest 2D texture side,
    /// and its largest buffer (a frame's readback).
    pub fn frame_limits(&self) -> (u32, u64) {
        let limits = self.device.limits();
        (limits.max_texture_dimension_2d, limits.max_buffer_size)
    }

    /// Render every frame at `width` × `height` until
    /// [`end_sized_frames`](Self::end_sized_frames).
    pub fn begin_sized_frames(&mut self, width: u32, height: u32) -> Result<(), String> {
        if width == 0 || height == 0 {
            return Err(format!("cannot render frames of {width}x{height}"));
        }
        let surface_size = self
            .sized_frames
            .take()
            .map_or((self.config.width, self.config.height), |s| s.surface_size);
        let frame = FrameTarget::new(&self.device, (width, height), self.config.format);
        let destination = self.frame_destination;
        self.sized_frames = Some(Box::new(SizedFrames {
            frame,
            surface_size,
            destination,
        }));
        self.set_frame_size(width, height);
        Ok(())
    }

    /// Back to frames the window's size.
    pub fn end_sized_frames(&mut self) {
        if let Some(sized) = self.sized_frames.take() {
            let (width, height) = sized.surface_size;
            self.set_frame_size(width, height);
            if let Some(surface) = &self.surface {
                surface.configure(&self.device, &self.config);
            }
        }
    }

    pub fn sized_frames_active(&self) -> bool {
        self.sized_frames.is_some()
    }

    /// The size frames render at while set-size frames are on.
    pub fn sized_frame_size(&self) -> Option<(u32, u32)> {
        self.sized_frames.as_ref().map(|sized| sized.frame.size)
    }

    /// Show set-size frames inside `rect` of the window (window px) instead
    /// of the whole window — a document's viewport. Kept across a restart
    /// of the frames.
    pub fn set_frame_destination(&mut self, rect: Option<[u32; 4]>) {
        self.frame_destination = rect;
        if let Some(sized) = self.sized_frames.as_mut() {
            sized.destination = rect;
        }
    }

    /// Draw this frame into the sized target; `present` shows it letterboxed
    /// in the window with the window's UI over it.
    pub(super) fn draw_sized_frame(
        &mut self,
        sized: &SizedFrames,
        present: bool,
    ) -> wgpu::SubmissionIndex {
        let swapchain = if present {
            self.acquire_swapchain_frame()
        } else {
            None
        };
        let window = swapchain.as_ref().map(|frame| {
            let size = frame.texture.size();
            (
                frame
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor::default()),
                (size.width, size.height),
            )
        });
        let submitted = self.encode_frame(FrameOut {
            scene: &sized.frame.view,
            scene_texture: Some(&sized.frame),
            window: window.as_ref().map(|(view, size)| WindowOut::Letterboxed {
                view,
                frame: &sized.frame,
                rect: sized.placement(*size),
            }),
        });
        if let Some(frame) = swapchain {
            frame.present();
        }
        submitted
    }

    /// The swapchain's configuration: the frame's, at the window's size.
    pub(super) fn surface_config(&self) -> wgpu::SurfaceConfiguration {
        let mut config = self.config.clone();
        if let Some(sized) = &self.sized_frames {
            (config.width, config.height) = sized.surface_size;
        }
        config
    }

    /// A window resize while sized frames are on: the swapchain follows the
    /// window, the frames keep their own size. `false` = not handled here.
    pub(super) fn resize_under_sized_frames(&mut self, width: u32, height: u32) -> bool {
        let Some(sized) = self.sized_frames.as_mut() else {
            return false;
        };
        sized.surface_size = (width, height);
        self.ui.window_generation = self.ui.window_generation.wrapping_add(1).max(1);
        if let Some(surface) = &self.surface {
            surface.configure(&self.device, &self.surface_config());
        }
        self.suboptimal_retried = false;
        true
    }

    fn set_frame_size(&mut self, width: u32, height: u32) {
        self.config.width = width;
        self.config.height = height;
        self.ui.scene_generation = self.ui.scene_generation.wrapping_add(1).max(1);
        self.recreate_scene_targets();
        self.chrome.crosshair_drawn_size = (0, 0);
    }
}

fn blit(
    device: &wgpu::Device,
    view: &wgpu::TextureView,
    format: wgpu::TextureFormat,
) -> (wgpu::RenderPipeline, wgpu::BindGroup) {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("frame blit"),
        source: wgpu::ShaderSource::Wgsl(BLIT_SHADER.into()),
    });
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("frame blit"),
        entries: &crate::pipeline::texture_sampler_layout_entries(
            0,
            wgpu::TextureViewDimension::D2,
        ),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("frame blit"),
        bind_group_layouts: &[&bgl],
        push_constant_ranges: &[],
    });
    let pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("frame blit"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_blit"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_blit"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
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
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("frame blit"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("frame blit"),
        layout: &bgl,
        entries: &crate::pipeline::texture_sampler_bind_entries(0, view, &sampler),
    });
    (pipe, bind)
}

#[cfg(test)]
mod tests {
    use super::letterbox;

    #[test]
    fn a_frame_is_shown_whole_and_centred_whatever_the_window() {
        // Wider window: pillarboxed.
        assert_eq!(
            letterbox((1920, 1080), (2560, 1080)),
            (320.0, 0.0, 1920.0, 1080.0)
        );
        // Taller window: letterboxed.
        assert_eq!(
            letterbox((1920, 1080), (1280, 1024)),
            (0.0, 152.0, 1280.0, 720.0)
        );
        // Same aspect: fills.
        assert_eq!(
            letterbox((640, 360), (1280, 720)),
            (0.0, 0.0, 1280.0, 720.0)
        );
    }
}
