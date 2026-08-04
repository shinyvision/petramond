//! Platform-specific entries: native desktop hosting and the headless
//! dedicated server.

pub mod server;

/// Shared logger setup for every runtime entry. Default (no RUST_LOG):
/// errors from everywhere plus petramond at info, so multiplayer lifecycle
/// lines (joins, leaves, kicks) show in a plain terminal. wgpu_hal's Vulkan
/// backend warns on EVERY suboptimal present — a permanent condition on some
/// Wayland/NVIDIA stacks even after a swapchain rebuild (see
/// `Renderer::render`) — so that module is clamped to errors even when
/// RUST_LOG opts into warns.
pub fn init_logging() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("error,petramond=info"),
    )
    .filter_module("wgpu_hal::vulkan", log::LevelFilter::Error)
    .init();
}
