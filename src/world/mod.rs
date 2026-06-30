//! World: manages loaded chunks, requests async generation, serves
//! neighbour-block queries for meshing.
//!
//! Gen is off-thread: see `worker` module. The facade keeps the public `World`
//! API stable while the implementation is split by responsibility.

mod chest;
pub(crate) mod door;
mod edit;
mod entities;
pub(crate) mod fragile;
mod furnace;
mod light_queue;
mod mesh_pool;
mod mesh_queue;
mod model;
mod query;
mod render_handoff;
pub(crate) mod sapling;
mod store;
mod stream;
mod tick;
mod torch;
pub(crate) mod water;

#[cfg(test)]
pub use entities::{ITEM_LIFETIME_TICKS, ITEM_PICKUP_DELAY_TICKS};

pub(crate) use render_handoff::{TerrainMeshUploadSource, TerrainRenderHandoff};
pub use store::{World, RENDER_DIST};
