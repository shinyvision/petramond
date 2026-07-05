//! World: manages loaded chunks, requests async generation, serves
//! neighbour-block queries for meshing.
//!
//! Gen is off-thread: see `worker` module. The facade keeps the public `World`
//! API stable while the implementation is split by responsibility.

mod chest;
pub(crate) mod door;
mod edit;
mod entities;
pub(crate) mod environment;
pub(crate) mod fragile;
mod furnace;
mod kv;
mod light;
mod mesh_pool;
mod mesh_queue;
mod model;
mod query;
mod render_handoff;
pub(crate) mod sapling;
mod sim_guard;
mod stair;
mod store;
mod stream;
mod tick;
mod torch;
mod visibility;
pub(crate) mod water;

#[cfg(test)]
pub use entities::{ITEM_LIFETIME_TICKS, ITEM_PICKUP_DELAY_TICKS};

pub(crate) use render_handoff::TerrainRenderHandoff;
pub(crate) use store::VERTICAL_LOAD_RADIUS;
pub use store::{World, RENDER_DIST};
pub(crate) use stream::StreamEvent;

/// Temporary perf-session diagnostics (see `tooling::stream::stage_stats`).
pub(crate) fn mesh_stage_stats() -> (
    &'static std::sync::atomic::AtomicU64,
    &'static std::sync::atomic::AtomicU64,
) {
    (&mesh_pool::MESH_STAGE_NS, &mesh_pool::MESH_STAGE_JOBS)
}

pub(crate) fn light_stage_stats() -> (
    &'static std::sync::atomic::AtomicU64,
    &'static std::sync::atomic::AtomicU64,
) {
    light::stage_stats()
}
