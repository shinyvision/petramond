use std::fmt;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug)]
pub enum RenderInitError {
    CreateSurface(wgpu::CreateSurfaceError),
    NoAdapter(wgpu::RequestAdapterError),
    RequestDevice(wgpu::RequestDeviceError),
    SurfaceUnsupported,
    UnreadableFormat(wgpu::TextureFormat),
    PassGraph(String),
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
                write!(
                    f,
                    "offscreen colour format {format:?} is not readable as 8-bit RGBA"
                )
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderFailure {
    DeviceLost(String),
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

const LOGGED_VALIDATION_ERRORS: u32 = 16;

#[derive(Clone, Default)]
pub(super) struct DeviceHealth {
    failure: Arc<Mutex<Option<RenderFailure>>>,
    validation_errors: Arc<AtomicU32>,
}

impl DeviceHealth {
    pub(super) fn watch(device: &wgpu::Device) -> Self {
        let health = Self::default();
        let lost = health.clone();
        device.set_device_lost_callback(move |reason, message| {
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

    pub(super) fn record(&self, failure: RenderFailure) {
        log::error!("renderer: {failure}");
        if let Ok(mut slot) = self.failure.lock() {
            slot.get_or_insert(failure);
        }
    }

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
