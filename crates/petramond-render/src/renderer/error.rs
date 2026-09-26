//! How the renderer fails: typed bring-up errors instead of panics, and the
//! runtime failures (device loss, out-of-memory) a host has to act on.
//!
//! wgpu reports device loss and uncaptured errors through callbacks that may
//! run on any thread and at any point of a frame, so they land in a shared
//! [`DeviceHealth`] record the frame loop and the host read back. Without the
//! callbacks installed an uncaptured error reaches wgpu's default handler,
//! which panics — a driver reset or one bad pack shader would take the whole
//! game down instead of letting the host rebuild or report.

use std::fmt;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

/// Why a renderer could not be built.
#[derive(Debug)]
pub enum RenderInitError {
    /// The window handle could not back a wgpu surface.
    CreateSurface(wgpu::CreateSurfaceError),
    /// No adapter — not even the software fallback — can drive the target.
    NoAdapter(wgpu::RequestAdapterError),
    /// The adapter refused the device the renderer needs.
    RequestDevice(wgpu::RequestDeviceError),
    /// The chosen adapter cannot present to the surface.
    SurfaceUnsupported,
    /// An offscreen renderer was asked for a colour format its frame
    /// readback cannot decode as 8-bit RGBA.
    UnreadableFormat(wgpu::TextureFormat),
    /// The pass table failed validation: a renderer bug, caught before the
    /// first frame rather than as a validation error in the middle of one.
    PassGraph(String),
    /// The loaded content (tiles, item icons, the uv table) overflows the
    /// GPU's limits; the message names every overflow.
    ContentLimits(String),
}

impl fmt::Display for RenderInitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CreateSurface(e) => write!(f, "cannot create a rendering surface: {e}"),
            Self::NoAdapter(e) => write!(f, "no compatible GPU adapter: {e}"),
            Self::RequestDevice(e) => write!(f, "the GPU refused the renderer's device: {e}"),
            Self::SurfaceUnsupported => {
                f.write_str("the GPU adapter cannot present to this window's surface")
            }
            Self::UnreadableFormat(format) => {
                write!(f, "offscreen colour format {format:?} is not readable as 8-bit RGBA")
            }
            Self::PassGraph(e) => write!(f, "invalid render pass graph: {e}"),
            Self::ContentLimits(e) => {
                write!(f, "the loaded content does not fit this GPU:\n  {e}")
            }
        }
    }
}

impl std::error::Error for RenderInitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::CreateSurface(e) => Some(e),
            Self::NoAdapter(e) => Some(e),
            Self::RequestDevice(e) => Some(e),
            Self::SurfaceUnsupported
            | Self::UnreadableFormat(_)
            | Self::PassGraph(_)
            | Self::ContentLimits(_) => None,
        }
    }
}

/// A failure the renderer cannot recover from on its own. Once one is
/// recorded the renderer stops drawing; the host decides what happens next
/// (see [`Renderer::failure`](super::Renderer::failure)).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderFailure {
    /// The GPU device is gone (driver reset, GPU removed or hung). Every GPU
    /// object this renderer holds is dead; build a new renderer — terrain
    /// re-uploads through `sync_meshes` once the world re-queues it.
    DeviceLost(String),
    /// The GPU ran out of memory for a frame or a resource.
    OutOfMemory(String),
}

impl fmt::Display for RenderFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DeviceLost(reason) => write!(f, "the GPU device was lost: {reason}"),
            Self::OutOfMemory(what) => write!(f, "the GPU ran out of memory: {what}"),
        }
    }
}

impl std::error::Error for RenderFailure {}

/// Validation errors logged in full before the rest are only counted: a
/// broken pipeline reports once per draw, and a log flooded at frame rate
/// buries the first (useful) message.
const LOGGED_VALIDATION_ERRORS: u32 = 16;

/// The device's failure state, shared with its lost and uncaptured-error
/// callbacks. Keeps the FIRST failure: later ones are usually consequences.
#[derive(Clone, Default)]
pub(super) struct DeviceHealth {
    failure: Arc<Mutex<Option<RenderFailure>>>,
    validation_errors: Arc<AtomicU32>,
}

impl DeviceHealth {
    /// Route `device`'s lost and uncaptured-error callbacks into a fresh
    /// health record.
    pub(super) fn watch(device: &wgpu::Device) -> Self {
        let health = Self::default();
        let lost = health.clone();
        device.set_device_lost_callback(move |reason, message| {
            // The renderer never destroys its device, so `Destroyed` only
            // arrives while the device is being torn down with the renderer.
            if reason == wgpu::DeviceLostReason::Destroyed {
                return;
            }
            lost.record(RenderFailure::DeviceLost(format!("{reason:?}: {message}")));
        });
        let uncaptured = health.clone();
        device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
            uncaptured.uncaptured(error);
        }));
        health
    }

    /// Record `failure` unless an earlier one is already recorded.
    pub(super) fn record(&self, failure: RenderFailure) {
        log::error!("renderer: {failure}");
        if let Ok(mut slot) = self.failure.lock() {
            slot.get_or_insert(failure);
        }
    }

    /// The recorded failure, if any.
    pub(super) fn failure(&self) -> Option<RenderFailure> {
        self.failure.lock().ok().and_then(|slot| slot.clone())
    }

    fn uncaptured(&self, error: wgpu::Error) {
        match error {
            wgpu::Error::OutOfMemory { source } => {
                self.record(RenderFailure::OutOfMemory(source.to_string()));
            }
            wgpu::Error::Validation { description, .. } => {
                let seen = self.validation_errors.fetch_add(1, Ordering::Relaxed);
                if seen < LOGGED_VALIDATION_ERRORS {
                    log::error!("wgpu validation error: {description}");
                } else if seen == LOGGED_VALIDATION_ERRORS {
                    log::error!("further wgpu validation errors are counted, not logged");
                }
                // A validation error is a renderer or pack-shader bug. Debug
                // builds (and every test) keep wgpu's loud default so a bug
                // fails where it happens; a release build logs and keeps
                // drawing whatever still validates.
                if cfg!(debug_assertions) {
                    panic!("wgpu validation error: {description}");
                }
            }
            wgpu::Error::Internal { description, .. } => {
                log::error!("wgpu internal error: {description}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_failure_wins() {
        let health = DeviceHealth::default();
        assert_eq!(health.failure(), None);
        health.record(RenderFailure::DeviceLost("reset".into()));
        health.record(RenderFailure::OutOfMemory("frame".into()));
        assert_eq!(
            health.failure(),
            Some(RenderFailure::DeviceLost("reset".into()))
        );
        // Reading does not consume: a lost device stays lost.
        assert!(health.failure().is_some());
    }

    #[test]
    fn clones_share_one_record() {
        let health = DeviceHealth::default();
        let callback_side = health.clone();
        callback_side.record(RenderFailure::OutOfMemory("texture".into()));
        assert_eq!(
            health.failure(),
            Some(RenderFailure::OutOfMemory("texture".into()))
        );
    }
}
