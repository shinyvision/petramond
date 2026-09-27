#[cfg(test)]
#[global_allocator]
static TEST_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

pub mod animation;
pub mod app;
mod folder_picker;
pub mod game;
pub mod keymap;
pub mod media;
pub mod native;
pub mod particle;
pub mod scene;

pub(crate) const PIXELS_PER_NOTCH: f32 = 120.0;
