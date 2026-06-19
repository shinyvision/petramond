//! WGPU renderer: atlas texture, opaque + transparent pipelines, fog.

mod pipeline;
mod renderer;
mod resources;
mod selection;
mod uniforms;

pub use renderer::{
    instance_descriptor, new_renderer, new_renderer_from_target, new_renderer_with_instance,
    Renderer,
};
pub use resources::GpuMesh;
pub use uniforms::{
    Uniforms, FOG_END, FOG_START, UNDERWATER_FOG_END, UNDERWATER_FOG_START, UV_RECTS_LEN,
};
