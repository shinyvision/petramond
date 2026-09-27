//! Frame captures: a frame's image copied out at its capture point and read
//! back without stalling the frame.
//!
//! A `Scene` capture copies the scene as the frame left it (world, hands, aim
//! marks, the scene's UI layer) before any window UI draws; at another size it
//! is a scaled blit of that copy. A `World` capture draws the frame's grade
//! pass again, from the same scene texture, into a target of its own size: the
//! graded world with no UI over it. A frame that captures draws its scene into
//! an owned target (the sized frames', or one the window's size) so there is
//! something to copy, and shows that target on the window.
//!
//! Every copy lands in a readback of the ring, mapped after submit and
//! collected with `PollType::Poll` on later frames. The ring holds as many
//! readbacks as frames the device can have in flight; a frame waits on the
//! oldest only when every slot is still on its way back.

use std::collections::VecDeque;

use super::frame::{FrameOut, WindowOut};
use super::offscreen::{RenderedFrame, CAPTURE_FORMATS};
use super::readback::Readback;
use super::sized_frames::FrameTarget;
use super::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CaptureSource {
    Scene,
    World,
}

/// One capture to take on the frame being drawn. `size: None` = the frame's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureRequest {
    pub id: u64,
    pub source: CaptureSource,
    pub size: Option<(u32, u32)>,
}

/// A capture's pixels, or why it could not be read.
pub struct Captured {
    pub id: u64,
    pub frame: Result<RenderedFrame, String>,
}

/// A capture-sized image the grade pass or the scaled blit draws into.
struct Target {
    size: (u32, u32),
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

struct Taking {
    id: u64,
    source: CaptureSource,
    target: Option<Target>,
    readback: Readback,
}

struct InFlight {
    id: u64,
    target: Option<Target>,
    readback: Readback,
}

#[derive(Default)]
pub(super) struct Captures {
    /// This frame's captures, copied at its capture point.
    taking: Vec<Taking>,
    /// Oldest first.
    in_flight: VecDeque<InFlight>,
    /// Results not handed out yet.
    done: Vec<Captured>,
    /// Readbacks and targets kept for the next capture of the same size.
    spare_readbacks: Vec<Readback>,
    spare_targets: Vec<Target>,
    /// The owned scene target of a capturing frame the window's size.
    window_target: Option<FrameTarget>,
    /// This frame takes a `World` capture, so its world passes through the
    /// scene texture and the grade pass.
    pub(super) world_due: bool,
}

/// Reuse keeps an export at a steady size from allocating per frame; what an
/// earlier size left behind goes once newer spares outnumber the ring.
fn keep_spare<T>(spares: &mut Vec<T>, spare: T, depth: usize) {
    spares.push(spare);
    if spares.len() > depth {
        spares.remove(0);
    }
}

impl Renderer {
    /// Readbacks the ring holds: the frames the device can have in flight,
    /// plus the one being recorded.
    fn capture_ring_depth(&self) -> usize {
        self.config.desired_maximum_frame_latency as usize + 1
    }

    /// Draw this frame, taking `captures` at its capture point; `present`
    /// shows it on the window. Returns the ids taken this frame (a failed
    /// take included, whose failure comes back from
    /// [`take_captured`](Self::take_captured)); the rest wait for a free ring
    /// slot. A frame with nothing to present and nothing to capture draws
    /// nothing.
    pub fn draw_frame(&mut self, captures: &[CaptureRequest], present: bool) -> Vec<u64> {
        let taken = self.prepare_captures(captures);
        if let Some(sized) = self.sized_frames.take() {
            if present || !self.captures.taking.is_empty() {
                let submitted = self.draw_sized_frame(&sized, present);
                self.map_captures(submitted);
            }
            self.sized_frames = Some(sized);
        } else if !self.captures.taking.is_empty() {
            let size = (self.config.width, self.config.height);
            let target = match self.captures.window_target.take() {
                Some(target) if target.size == size => target,
                _ => FrameTarget::new(&self.device, size, self.config.format),
            };
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
                scene: &target.view,
                scene_texture: Some(&target),
                window: window.as_ref().map(|(view, size)| WindowOut::Letterboxed {
                    view,
                    frame: &target,
                    rect: sized_frames::letterbox(target.size, *size),
                }),
            });
            if let Some(frame) = swapchain {
                frame.present();
            }
            self.map_captures(submitted);
            self.captures.window_target = Some(target);
        } else if present {
            self.render();
        }
        self.collect_captures(false);
        taken
    }

    /// Every capture result since the last call: failures, and the reads that
    /// have come back, oldest first. Never blocks.
    pub fn take_captured(&mut self) -> Vec<Captured> {
        self.collect_captures(false);
        std::mem::take(&mut self.captures.done)
    }

    /// Every capture still on its way back, waited for.
    pub fn finish_captures(&mut self) -> Vec<Captured> {
        self.collect_captures(true);
        std::mem::take(&mut self.captures.done)
    }

    /// Why no capture can be read from this renderer, if none can.
    pub fn capture_refusal(&self) -> Option<String> {
        let format = self.config.format;
        (!CAPTURE_FORMATS.contains(&format))
            .then(|| format!("the colour format {format:?} cannot be read back as 8-bit RGBA"))
    }

    /// Captures on their way back from the GPU.
    pub fn captures_in_flight(&self) -> usize {
        self.captures.in_flight.len()
    }

    fn prepare_captures(&mut self, captures: &[CaptureRequest]) -> Vec<u64> {
        if captures.is_empty() {
            return Vec::new();
        }
        if let Some(why) = self.capture_refusal() {
            for request in captures {
                self.captures.done.push(Captured {
                    id: request.id,
                    frame: Err(why.clone()),
                });
            }
            return captures.iter().map(|r| r.id).collect();
        }
        let depth = self.capture_ring_depth();
        if self.captures.in_flight.len() >= depth {
            self.collect_oldest_capture(true);
        }
        let slots = depth.saturating_sub(self.captures.in_flight.len());
        let mut taken = Vec::new();
        for request in captures.iter().take(slots) {
            taken.push(request.id);
            match self.capture_resources(request) {
                Ok((target, readback)) => self.captures.taking.push(Taking {
                    id: request.id,
                    source: request.source,
                    target,
                    readback,
                }),
                Err(why) => self.captures.done.push(Captured {
                    id: request.id,
                    frame: Err(why),
                }),
            }
        }
        self.captures.world_due = self
            .captures
            .taking
            .iter()
            .any(|t| t.source == CaptureSource::World);
        taken
    }

    /// The target (none for a `Scene` capture at the frame's own size) and the
    /// readback of one capture, allocated inside an out-of-memory scope so a
    /// frame too large for the GPU fails that capture, never the device.
    fn capture_resources(
        &mut self,
        request: &CaptureRequest,
    ) -> Result<(Option<Target>, Readback), String> {
        let frame_size = (self.config.width, self.config.height);
        let size = request.size.unwrap_or(frame_size);
        if size.0 == 0 || size.1 == 0 {
            return Err(format!("cannot capture a {}x{} frame", size.0, size.1));
        }
        let format = self.config.format;
        let needs_target = request.source == CaptureSource::World || size != frame_size;
        self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let target = needs_target.then(|| {
            match self
                .captures
                .spare_targets
                .iter()
                .position(|t| t.size == size)
            {
                Some(at) => self.captures.spare_targets.swap_remove(at),
                None => {
                    let texture = crate::gpu_mem::create_texture(
                        &self.device,
                        &wgpu::TextureDescriptor {
                            label: Some("frame capture"),
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
                                | wgpu::TextureUsages::COPY_SRC,
                            view_formats: &[],
                        },
                    );
                    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                    Target {
                        size,
                        texture,
                        view,
                    }
                }
            }
        });
        let readback = match self
            .captures
            .spare_readbacks
            .iter()
            .position(|r| r.size() == size)
        {
            Some(at) => self.captures.spare_readbacks.swap_remove(at),
            None => Readback::new(&self.device, size, format, "frame capture readback"),
        };
        match pollster::block_on(self.device.pop_error_scope()) {
            None => Ok((target, readback)),
            Some(error) => Err(format!(
                "the GPU could not hold a {}x{} capture: {error}",
                size.0, size.1
            )),
        }
    }

    /// At the capture point: every capture of this frame copied out of
    /// `scene`, the texture the frame's scene just landed in.
    pub(super) fn encode_captures(&mut self, enc: &mut wgpu::CommandEncoder, scene: &FrameTarget) {
        let taking = std::mem::take(&mut self.captures.taking);
        for capture in &taking {
            match (&capture.target, capture.source) {
                (Some(target), CaptureSource::World) => {
                    let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("world capture"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &target.view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        ..Default::default()
                    });
                    pass.set_pipeline(&self.targets.grade_pipe);
                    pass.set_bind_group(0, &self.targets.grade_bind, &[]);
                    pass.draw(0..3, 0..1);
                }
                (Some(target), CaptureSource::Scene) => {
                    let (w, h) = target.size;
                    scene.blit_into(
                        enc,
                        &target.view,
                        (0.0, 0.0, w as f32, h as f32),
                        "scene capture",
                    );
                }
                (None, _) => {}
            }
            let source = capture
                .target
                .as_ref()
                .map_or(&scene.texture, |t| &t.texture);
            capture.readback.record(enc, source);
        }
        self.captures.taking = taking;
        self.captures.world_due = false;
    }

    fn map_captures(&mut self, submitted: wgpu::SubmissionIndex) {
        for mut capture in std::mem::take(&mut self.captures.taking) {
            capture.readback.map(submitted.clone());
            self.captures.in_flight.push_back(InFlight {
                id: capture.id,
                target: capture.target,
                readback: capture.readback,
            });
        }
    }

    fn collect_captures(&mut self, wait: bool) {
        if self.captures.in_flight.is_empty() {
            return;
        }
        let _ = self.device.poll(wgpu::PollType::Poll);
        while self.collect_oldest_capture(wait) {}
    }

    /// Collect the oldest capture on its way back if it has landed (`wait`:
    /// once it has). `false` = none was collected.
    fn collect_oldest_capture(&mut self, wait: bool) -> bool {
        let Some(oldest) = self.captures.in_flight.front() else {
            return false;
        };
        let frame = match oldest.readback.collect(&self.device, wait) {
            Ok(None) => return false,
            Ok(Some(frame)) => Ok(frame),
            Err(e) => Err(e),
        };
        let Some(landed) = self.captures.in_flight.pop_front() else {
            return false;
        };
        let depth = self.capture_ring_depth();
        keep_spare(&mut self.captures.spare_readbacks, landed.readback, depth);
        if let Some(target) = landed.target {
            keep_spare(&mut self.captures.spare_targets, target, depth);
        }
        self.captures.done.push(Captured {
            id: landed.id,
            frame,
        });
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{CaptureRequest, CaptureSource};
    use crate::{ClientOverlayLayer, DocumentUiFrame, UiFrame, UiLayers};
    use petramond::gui::UiSnapshot;
    use petramond_world::gui_state::GuiKind;

    /// A full-frame solid of `rgb` for one UI layer's document.
    fn solid(size: (u32, u32), rgb: [f32; 3]) -> petramond_ui::DrawList {
        let mut list = petramond_ui::DrawList::default();
        let font = petramond_ui::text::Font::builtin();
        let mut painter = petramond_ui::Painter {
            list: &mut list,
            scale: 1,
            font: &font,
        };
        let rect = petramond_ui::RectI {
            x: 0,
            y: 0,
            w: size.0 as i32,
            h: size.1 as i32,
        };
        painter.solid(rect, [rgb[0], rgb[1], rgb[2], 1.0], None);
        list
    }

    fn layer<'a>(
        viewport: petramond::gui::UiViewport,
        draw: &'a petramond_ui::DrawList,
        content: &'a UiSnapshot,
        overlays: &'a ClientOverlayLayer,
    ) -> UiFrame<'a> {
        UiFrame {
            viewport,
            document: Some(DocumentUiFrame {
                viewport,
                kind: content.kind,
                draw,
                images: &[],
                slots: &[],
                hooks: &[],
            }),
            content,
            client_overlays: overlays,
            client_overlay_dim: false,
        }
    }

    #[test]
    fn captures_come_back_once_in_order_holding_the_scene_and_never_the_window() {
        let instance = wgpu::Instance::new(&crate::renderer::instance_descriptor());
        if pollster::block_on(instance.request_adapter(&Default::default())).is_err() {
            eprintln!("[skip] no wgpu adapter; capture readback not run");
            return;
        }
        // With a frame size of its own, and at the window's.
        for sized in [true, false] {
            let mut renderer = pollster::block_on(crate::new_offscreen_renderer(
                64,
                36,
                wgpu::TextureFormat::Rgba8UnormSrgb,
            ))
            .expect("offscreen renderer");
            if sized {
                renderer.begin_sized_frames(32, 18).unwrap();
            }
            let frame = renderer.scene_ui_viewport().size;
            let content = UiSnapshot {
                kind: GuiKind::Hotbar,
                ..Default::default()
            };
            let overlays = ClientOverlayLayer::default();
            let window = solid((64, 36), [1.0, 0.0, 0.0]);
            let frames = renderer.capture_ring_depth() as u64 * 4;
            let mut asked = Vec::new();
            let mut back = Vec::new();
            for i in 0..frames {
                let shade = (i + 1) as f32 / (frames + 1) as f32;
                let scene = solid(frame, [shade; 3]);
                assert!(renderer.prepare_ui_frame(UiLayers {
                    scene: layer(renderer.scene_ui_viewport(), &scene, &content, &overlays),
                    window: layer(renderer.window_ui_viewport(), &window, &content, &overlays),
                }));
                // Every third frame is only drawn: a held frame reads nothing.
                let captures: Vec<CaptureRequest> = (i % 3 != 2)
                    .then(|| CaptureRequest {
                        id: i,
                        source: CaptureSource::Scene,
                        // Every other capture at half size: the scaled route.
                        size: (i % 2 == 1).then_some((frame.0 / 2, frame.1 / 2)),
                    })
                    .into_iter()
                    .collect();
                let taken = renderer.draw_frame(&captures, true);
                assert_eq!(taken.len(), captures.len(), "a free slot takes the capture");
                asked.extend(taken);
                back.extend(renderer.take_captured());
            }
            back.extend(renderer.finish_captures());
            let ids: Vec<u64> = back.iter().map(|c| c.id).collect();
            assert_eq!(ids, asked, "every capture came back exactly once, in order");
            let pixels: Vec<[u8; 4]> = back
                .iter()
                .map(|c| {
                    let captured = c.frame.as_ref().unwrap();
                    let want = if c.id % 2 == 1 {
                        (frame.0 / 2, frame.1 / 2)
                    } else {
                        frame
                    };
                    assert_eq!((captured.width, captured.height), want);
                    captured.rgba[..4].try_into().unwrap()
                })
                .collect();
            assert!(
                pixels.iter().all(|[r, g, b, _]| r == g && g == b),
                "the window's UI reached a capture (sized: {sized}): {pixels:?}"
            );
            assert!(
                pixels.windows(2).all(|pair| pair[0][0] < pair[1][0]),
                "captures came back out of order (sized: {sized}): {pixels:?}"
            );
        }
    }
}
