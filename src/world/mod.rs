//! World: manages loaded chunks, requests async generation, serves
//! neighbour-block queries for meshing.
//!
//! Gen is off-thread: see `worker` module. The facade keeps the public `World`
//! API stable while the implementation is split by responsibility.

mod block_deltas;
pub(crate) mod chest;
mod column_heightmaps;
mod container;
mod cursor;
mod custom_bake;
pub(crate) mod door;
pub mod draw;
mod edit;
mod entities;
pub(crate) mod environment;
mod fence;
pub(crate) mod fragile;
mod furnace;
mod invalidation;
mod kv;
mod ladder;
mod light;
mod load_targets;
mod mesh_pool;
mod mesh_queue;
mod mobs;
mod model;
mod neighborhood;
mod pane;
mod particle_emitters;
pub(crate) mod placement;
mod prediction_render;
mod query;
mod remote;
mod render_handoff;
pub(crate) mod sapling;
pub(crate) mod shape_bake_validate;
mod shape_refine;
mod sim_guard;
mod slab;
mod snapshot;
mod stair;
mod store;
mod stream;
mod surface_tint;
mod tick;
mod torch;
mod visibility;
pub(crate) mod water;

pub(crate) use cursor::SectionCursor;
pub(crate) use custom_bake::CustomBakeCell;
#[cfg(test)]
pub use entities::{ITEM_LIFETIME_TICKS, ITEM_PICKUP_DELAY_TICKS};
pub(crate) use shape_bake_validate::ingest_shape_boxes;
#[cfg(test)]
pub(crate) use stream::split_generated_column;

pub use ladder::Climb;
pub use particle_emitters::{emitter_envelope, PlacedEmitter};
pub use query::CollisionShapeClass;
pub(crate) use render_handoff::TerrainRenderHandoff;
pub(crate) use store::VERTICAL_LOAD_RADIUS;
pub(crate) use store::{LoadAnchor, WorldRole};
pub use store::{MemoryCensus, World, RENDER_DIST};
pub(crate) use stream::StreamEvent;

#[cfg(test)]
pub(crate) mod testutil {
    use crate::block::Block;
    use crate::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};

    use super::store::World;

    /// A world with a 3×3 block of loaded chunks around the origin, a solid
    /// stone floor at y=64, air above.
    pub(crate) fn flat_world() -> World {
        let mut w = World::new(0, 1);
        for cz in -1..=1 {
            for cx in -1..=1 {
                let mut c = Chunk::new(cx, cz);
                for z in 0..CHUNK_SZ {
                    for x in 0..CHUNK_SX {
                        c.set_block(x, 64, z, Block::Stone);
                    }
                }
                w.insert_chunk_for_test(ChunkPos::new(cx, cz), c);
            }
        }
        w
    }
}
