//! World: the cubic voxel world and the orchestration around it, as two
//! types sharing one deterministic core — [`ServerWorld`] (generation,
//! simulation, replication capture, persistence) and [`ReplicaWorld`]
//! (installs from the connection, meshes for the renderer). `side` says what
//! each owns; [`World::data`] is the read-only query surface both share.
//!
//! Gen is off-thread: see the `worker` module.

// The data half (WorldData + pure-data query modules) lives in
// `petramond_world::world`; this module layers orchestration on top.
pub use petramond_world::world::data::WorldData;
pub use petramond_world::world::{
    data, environment, load_targets, placement as placement_types, shape_bake_validate, tick_state,
};

pub mod actor;
pub mod animated_block;
mod block_deltas;
pub(crate) mod cells;
pub mod chest;
mod column_heightmaps;
pub mod construction;
mod content;
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
pub mod mirror;
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
pub mod replication;
pub mod sapling;
pub(crate) mod schematic;
pub mod session;
mod shape_refine;
mod side;
mod sim_guard;
mod slab;
mod snapshot;
mod stair;
mod store;
mod stream;

pub use petramond_world::world::SavedIndex;
pub mod fluid;
mod surface_tint;
mod tick;
pub mod trapdoor;
mod visibility;

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
pub use mirror::ReplicaMirror;
pub use side::{ReplicaSide, ServerSide, WorldSide};
pub use store::{MemoryCensus, ReplicaWorld, ServerWorld, World, RENDER_DIST};
pub use stream::StreamEvent;
pub use tick::TICK_DT;

#[cfg(any(test, feature = "test-support"))]
pub mod testutil {
    //! World fixtures, one per side of the split: sim, streaming and
    //! replication tests build a [`ServerWorld`], presentation tests a
    //! [`ReplicaWorld`].

    use petramond_world::block::Block;
    use petramond_world::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};

    use super::side::WorldSide;
    use super::store::{ReplicaWorld, ServerWorld, World};

    /// Install a 3×3 block of chunks around the origin with a solid stone
    /// floor at y=64, air above.
    pub fn install_flat_floor<S: WorldSide>(w: &mut World<S>) {
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
    }

    /// A server world over the [`install_flat_floor`] fixture.
    pub fn flat_server_world() -> ServerWorld {
        let mut w = ServerWorld::new(0, 1);
        install_flat_floor(&mut w);
        w
    }

    /// A replica over the [`install_flat_floor`] fixture.
    pub fn flat_replica_world() -> ReplicaWorld {
        let mut w = ReplicaWorld::new(0, 1);
        install_flat_floor(&mut w);
        w
    }
}
