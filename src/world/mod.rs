//! World: manages loaded chunks, requests async generation, serves
//! neighbour-block queries for meshing.
//!
//! Gen is off-thread: see `worker` module. The facade keeps the public `World`
//! API stable while the implementation is split by responsibility.

// The data half (WorldData + pure-data query modules) lives in
// `petramond_world::world`; this module layers orchestration on top.
pub use petramond_world::world::data::{WorldData, WorldRole};
pub use petramond_world::world::{
    data, environment, load_targets, placement as placement_types, shape_bake_validate, tick_state,
};

mod block_deltas;
pub mod chest;
mod column_heightmaps;
mod container;
mod cursor;
mod custom_bake;
pub mod door;
pub mod draw;
mod edit;
pub(crate) mod engine_behavior;
mod entities;
pub mod fragile;
mod furnace;
mod invalidation;
mod kv;
mod light;
mod mesh_pool;
mod mesh_queue;
mod mobs;
mod model;
mod particle_emitters;
pub mod placement;
mod prediction_render;
mod query;
#[cfg(test)]
mod relocated_world_crate_tests;
mod remote;
mod render_handoff;
pub mod sapling;
mod shape_refine;
mod sim_guard;
mod slab;
mod snapshot;
mod stair;
mod store;
mod stream;

pub use petramond_world::world::SavedIndex;
mod surface_tint;
mod tick;
mod visibility;
pub mod water;

pub use cursor::SectionCursor;
pub use entities::{ImpactTarget, ItemImpact, ItemStep, ITEM_MERGE_INTERVAL_TICKS};
#[cfg(any(test, feature = "test-support"))]
pub use entities::{ITEM_LIFETIME_TICKS, ITEM_PICKUP_DELAY_TICKS};
pub use petramond_world::world::custom_bake::CustomBakeCell;
pub use petramond_world::world::shape_bake_validate::ingest_shape_boxes;
#[cfg(any(test, feature = "test-support"))]
pub use stream::split_generated_column;

pub use particle_emitters::{emitter_envelope, PlacedEmitter};
pub use petramond_world::world::ladder::Climb;
pub use petramond_world::world::query::CollisionShapeClass;
pub use render_handoff::TerrainRenderHandoff;
pub use store::LoadAnchor;
pub use store::VERTICAL_LOAD_RADIUS;
pub use store::{MemoryCensus, World, RENDER_DIST};
pub use stream::StreamEvent;

#[cfg(any(test, feature = "test-support"))]
pub mod testutil {
    use petramond_world::block::Block;
    use petramond_world::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};

    use super::store::World;

    /// A world with a 3×3 block of loaded chunks around the origin, a solid
    /// stone floor at y=64, air above.
    pub fn flat_world() -> World {
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
