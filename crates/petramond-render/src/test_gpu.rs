use std::sync::{Mutex, MutexGuard};

static GPU: Mutex<()> = Mutex::new(());

/// Held by every test that creates a wgpu device, for as long as the device
/// lives. CI renders on Mesa's software Vulkan driver, which crashes the test
/// process when devices are created and torn down on several threads at once.
pub fn serial() -> MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
