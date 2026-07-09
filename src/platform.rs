//! Platform-specific entries: native desktop hosting and the headless
//! dedicated server.

/// Pixels of high-precision scroll (`PixelDelta`) that equal one wheel notch.
/// Mirrors Windows' `WHEEL_DELTA` (120) so a pixel-reporting device — a trackpad
/// or a hi-res / free-spin wheel like the MX Master — needs the same deliberate
/// travel per hotbar slot as a classic detented wheel, instead of skidding
/// across the bar on the tiniest nudge.
pub(crate) const PIXELS_PER_NOTCH: f32 = 120.0;

pub mod native;
pub mod server;

/// Shared logger setup for every runtime entry. Default (no RUST_LOG):
/// errors from everywhere plus petramond at info, so multiplayer lifecycle
/// lines (joins, leaves, kicks) show in a plain terminal. wgpu_hal's Vulkan
/// backend warns on EVERY suboptimal present — a permanent condition on some
/// Wayland/NVIDIA stacks even after a swapchain rebuild (see
/// `Renderer::render`) — so that module is clamped to errors even when
/// RUST_LOG opts into warns.
pub(crate) fn init_logging() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("error,petramond=info"),
    )
    .filter_module("wgpu_hal::vulkan", log::LevelFilter::Error)
    .init();
}
