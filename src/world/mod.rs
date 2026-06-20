//! World: manages loaded chunks, requests async generation, serves
//! neighbour-block queries for meshing.
//!
//! Gen is off-thread: see `worker` module. The facade keeps the public `World`
//! API stable while the implementation is split by responsibility.

mod edit;
mod mesh_queue;
mod query;
mod store;
mod stream;
mod visibility;

pub use query::WorldQuery;
pub use store::{World, RENDER_DIST};
pub use visibility::{SectionConnectivity, SectionFace, SectionPos, SECTION_FACES};
