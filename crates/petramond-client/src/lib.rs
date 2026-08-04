//! Petramond client: the windowed app shell, the client game session
//! (replica, prediction, presentation), the native platform host, and the
//! offscreen scene harness dev tools ride on.

#![allow(clippy::too_many_arguments)]

pub mod app;
pub mod game;
pub mod keymap;
pub mod native;
pub mod particle;
pub mod scene;

/// Pixels of high-precision scroll (`PixelDelta`) that equal one wheel notch.
/// Mirrors Windows' `WHEEL_DELTA` (120) so a pixel-reporting device — a trackpad
/// or a hi-res / free-spin wheel like the MX Master — needs the same deliberate
/// travel per hotbar slot as a classic detented wheel, instead of skidding
/// across the bar on the tiniest nudge.
pub(crate) const PIXELS_PER_NOTCH: f32 = 120.0;
