//! World: manages loaded chunks, requests async generation, serves
//! neighbour-block queries for meshing.
//!
//! Gen is off-thread: see `worker` module. The facade keeps the public `World`
//! API stable while the implementation is split by responsibility.

mod chest;
mod edit;
mod entities;
pub(crate) mod fragile;
mod furnace;
mod light_queue;
mod mesh_queue;
mod model;
mod query;
mod store;
mod stream;
mod tick;
mod torch;
mod visibility;
pub(crate) mod water;

pub use entities::{DroppedItems, ITEM_LIFETIME_TICKS, ITEM_PICKUP_DELAY_TICKS};

pub use store::{World, RENDER_DIST};
pub use visibility::{SectionConnectivity, SectionFace, SectionPos, SECTION_FACES};
