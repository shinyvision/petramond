//! Windowless rendering: a [`Renderer`] with no swapchain, plus one-frame
//! capture straight to CPU pixels.
//!
//! A capture draws through the SAME `plan_draw_order` + frame graph the
//! windowed game draws through — only the colour target differs — so the image
//! is what the window would have shown. That makes it usable for looking at
//! generated content without launching the game, and for visual regression
//! tests later.

use super::*;

/// One rendered frame on the CPU: `width * height` tightly packed RGBA8
/// pixels, top row first. Row padding from the GPU copy alignment is already
/// stripped, and BGRA targets are already swizzled.
pub struct RenderedFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Colour formats [`Renderer::capture_frame`] can read back: 8-bit, four
/// channels, RGBA or BGRA order. Anything wider would silently skew the
/// readback, which assumes [`TEXEL_BYTES`] per pixel.
pub(super) const CAPTURE_FORMATS: [wgpu::TextureFormat; 4] = [
    wgpu::TextureFormat::Rgba8Unorm,
    wgpu::TextureFormat::Rgba8UnormSrgb,
    wgpu::TextureFormat::Bgra8Unorm,
    wgpu::TextureFormat::Bgra8UnormSrgb,
];

const TEXEL_BYTES: u32 = 4;

/// Build a renderer with no surface at `width` × `height`, rendering in
/// `format` (one of `CAPTURE_FORMATS`), or why it cannot be built. Prefer an
/// sRGB format: every pipeline (and the pre-baked icon atlas) is built for the
/// colour format handed in here, and the windowed game runs on an sRGB
/// swapchain.
pub async fn new_offscreen_renderer(
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> Result<Renderer, RenderInitError> {
    if !CAPTURE_FORMATS.contains(&format) {
        return Err(RenderInitError::UnreadableFormat(format));
    }
    let instance = wgpu::Instance::new(&super::construct::instance_descriptor());
    let adapter = super::construct::request_adapter(&instance, None).await?;
    let (device, queue) = super::construct::request_device(&adapter).await?;
    // Present-only fields (`present_mode`, `alpha_mode`) are inert without a
    // swapchain; `config` is the renderer's frame geometry + format either way.
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: width.max(1),
        height: height.max(1),
        present_mode: wgpu::PresentMode::Fifo,
        desired_maximum_frame_latency: 2,
        alpha_mode: wgpu::CompositeAlphaMode::Auto,
        view_formats: vec![],
    };
    super::construct::new_renderer_inner(
        None,
        device,
        queue,
        config,
        super::construct::max_scene_samples(&adapter, format),
    )
}

impl Renderer {
    /// Encode + submit one frame into a REUSED offscreen target, with no
    /// readback. The frame-cost instrument: `capture_frame`'s per-call texture,
    /// staging buffer and copy would dominate any repeated timing.
    pub fn render_offscreen(&mut self) {
        let (width, height) = (self.config.width, self.config.height);
        if self.offscreen_target.as_ref().map(|(w, h, _)| (*w, *h)) != Some((width, height)) {
            let target = crate::gpu_mem::create_texture(
                &self.device,
                &wgpu::TextureDescriptor {
                    label: Some("offscreen frame (reused)"),
                    size: wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: self.config.format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                },
            );
            let view = target.create_view(&wgpu::TextureViewDescriptor::default());
            self.offscreen_target = Some((width, height, view));
        }
        let (_, _, view) = self.offscreen_target.take().expect("offscreen target");
        self.encode_frame(super::frame::FrameOut {
            scene: &view,
            scene_texture: None,
            window: Some(super::frame::WindowOut::Scene),
        });
        self.offscreen_target = Some((width, height, view));
    }

    /// Render one frame into an owned offscreen target and read it back: the
    /// window as it would show, the scene with the window's UI over it.
    /// Blocks until the GPU is done. Works with or without a surface; nothing
    /// is presented either way. Panics unless the renderer's colour format is
    /// one of `CAPTURE_FORMATS` — a windowed renderer on an HDR swapchain is
    /// not capturable.
    pub fn capture_frame(&mut self) -> RenderedFrame {
        let (width, height) = (self.config.width, self.config.height);
        let format = self.config.format;
        assert!(
            CAPTURE_FORMATS.contains(&format),
            "colour format {format:?} is not readable as 8-bit RGBA"
        );
        let target = crate::gpu_mem::create_texture(
            &self.device,
            &wgpu::TextureDescriptor {
                label: Some("offscreen frame"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            },
        );
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        self.encode_frame(super::frame::FrameOut {
            scene: &view,
            scene_texture: None,
            window: Some(super::frame::WindowOut::Scene),
        });

        let mut readback = super::readback::Readback::new(
            &self.device,
            (width, height),
            format,
            "offscreen readback",
        );
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("offscreen readback"),
            });
        readback.record(&mut enc, &target);
        let submitted = self.queue.submit(std::iter::once(enc.finish()));
        readback.map(submitted);
        readback
            .collect(&self.device, true)
            .expect("offscreen readback")
            .expect("a waited readback has landed")
    }
}

/// Repack a `copy_texture_to_buffer` readback as tightly packed RGBA8: every
/// source row is padded up to [`wgpu::COPY_BYTES_PER_ROW_ALIGNMENT`], and a
/// BGRA target needs red and blue swapped. Getting either wrong yields a frame
/// that is skewed or channel-swapped yet still plausible, so this is kept apart
/// from the GPU work and tested.
pub(super) fn pack_rows(
    mapped: &[u8],
    width: u32,
    height: u32,
    padded_row: u32,
    bgra: bool,
) -> Vec<u8> {
    let unpadded_row = (width * TEXEL_BYTES) as usize;
    let mut out = Vec::with_capacity(unpadded_row * height as usize);
    for row in 0..height as usize {
        let start = row * padded_row as usize;
        out.extend_from_slice(&mapped[start..start + unpadded_row]);
    }
    if bgra {
        for px in out.chunks_exact_mut(TEXEL_BYTES as usize) {
            px.swap(0, 2);
        }
    }
    out
}

#[cfg(test)]
mod tests;
