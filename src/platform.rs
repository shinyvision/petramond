pub mod server;

pub fn init_logging() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("error,petramond=info"),
    )
    .filter_module("wgpu_hal::vulkan", log::LevelFilter::Error)
    .init();
}
