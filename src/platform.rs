//! Platform-specific entries.

/// Pixels of high-precision scroll (`PixelDelta` on native, `DOM_DELTA_PIXEL` on
/// web) that equal one wheel notch. Mirrors Windows' `WHEEL_DELTA` (120) so a
/// pixel-reporting device — a trackpad or a hi-res / free-spin wheel like the MX
/// Master — needs the same deliberate travel per hotbar slot as a classic
/// detented wheel, instead of skidding across the bar on the tiniest nudge.
pub(crate) const PIXELS_PER_NOTCH: f32 = 120.0;

#[cfg(not(target_arch = "wasm32"))]
pub mod native;

#[cfg(target_arch = "wasm32")]
pub mod web;
