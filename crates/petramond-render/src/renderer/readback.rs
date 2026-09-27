//! A texture's pixels on their way back to the CPU: one buffer sized for the
//! texture, copied into in a frame's encoder, mapped once that frame is
//! submitted, and handed back as tightly packed RGBA8. Every frame read —
//! a one-off capture, a stream of set-size frames, a world still — goes
//! through it.

use std::sync::{Arc, Mutex};

use super::offscreen::{pack_rows, RenderedFrame};

const TEXEL_BYTES: u32 = 4;

/// Where a mapping's outcome lands, set from wgpu's callback.
type MapOutcome = Arc<Mutex<Option<Result<(), wgpu::BufferAsyncError>>>>;

pub(super) struct Readback {
    buffer: wgpu::Buffer,
    size: (u32, u32),
    padded_row: u32,
    bgra: bool,
    mapped: MapOutcome,
    submitted: Option<wgpu::SubmissionIndex>,
}

impl Readback {
    /// A readback for a `size` texture of `format` (one of the capture
    /// formats).
    pub(super) fn new(
        device: &wgpu::Device,
        size: (u32, u32),
        format: wgpu::TextureFormat,
        label: &str,
    ) -> Self {
        let padded_row =
            (size.0 * TEXEL_BYTES).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        Readback {
            buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: u64::from(padded_row) * u64::from(size.1),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
            size,
            padded_row,
            bgra: matches!(
                format,
                wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
            ),
            mapped: MapOutcome::default(),
            submitted: None,
        }
    }

    pub(super) fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Copy `texture` (this readback's size) in, in `enc`'s order.
    pub(super) fn record(&self, enc: &mut wgpu::CommandEncoder, texture: &wgpu::Texture) {
        enc.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &self.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_row),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: self.size.0,
                height: self.size.1,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Start mapping once the copy's frame is submitted as `submitted`.
    pub(super) fn map(&mut self, submitted: wgpu::SubmissionIndex) {
        let outcome = Arc::clone(&self.mapped);
        *outcome.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                *outcome.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
            });
        self.submitted = Some(submitted);
    }

    /// The pixels, once the mapping has landed (`wait`: once it has). The
    /// caller polls the device when it does not wait.
    pub(super) fn collect(
        &self,
        device: &wgpu::Device,
        wait: bool,
    ) -> Result<Option<RenderedFrame>, String> {
        let landed = || self.mapped.lock().unwrap_or_else(|e| e.into_inner()).take();
        let outcome = match landed() {
            Some(outcome) => outcome,
            None if wait => {
                device
                    .poll(wgpu::PollType::Wait {
                        submission_index: self.submitted.clone(),
                        timeout: None,
                    })
                    .map_err(|e| format!("frame readback: {e}"))?;
                landed().ok_or("frame readback never completed")?
            }
            None => return Ok(None),
        };
        outcome.map_err(|e| format!("frame readback: {e}"))?;
        let (width, height) = self.size;
        let rgba = {
            let mapped = self.buffer.slice(..).get_mapped_range();
            pack_rows(&mapped, width, height, self.padded_row, self.bgra)
        };
        self.buffer.unmap();
        Ok(Some(RenderedFrame {
            width,
            height,
            rgba,
        }))
    }
}
