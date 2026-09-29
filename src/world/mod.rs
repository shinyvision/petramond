pub use petramond_world::world::data::WorldData;
pub use petramond_world::world::{
    data, environment, load_targets, placement as placement_types, shape_bake_validate, tick_state,
};

pub mod actor;
pub mod animated_block;
mod block_deltas;
mod cell_change;
pub(crate) mod cells;
pub mod chest;
mod column_heightmaps;
pub mod construction;
mod container;
mod content;
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
pub use remote::{
    column_key, decode_section_payload, section_key, Cached, Changes, DetachedFold, FrameChanges,
    PieceRange, Resident, SectionContent, SendEvents, SentSections, TerrainEdit, TerrainSendPlan,
};
#[cfg(any(test, feature = "test-support"))]
pub use stream::split_generated_column;

pub use mirror::ReplicaMirror;
pub use particle_emitters::{emitter_envelope, PlacedEmitter};
pub use petramond_world::world::ladder::Climb;
pub use petramond_world::world::query::CollisionShapeClass;
pub(crate) use remote::{detached_column_payload, detached_section_payload};
pub use render_handoff::TerrainRenderHandoff;
pub use side::{ReplicaSide, ServerSide, WorldSide};
pub use store::LoadAnchor;
pub use store::VERTICAL_LOAD_RADIUS;
pub use store::{MemoryCensus, ReplicaWorld, ServerWorld, World, RENDER_DIST};
pub use stream::StreamEvent;
pub use tick::TICK_DT;

#[cfg(any(test, feature = "test-support"))]
pub mod testutil {

    use petramond_world::block::Block;
    use petramond_world::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};

    use super::side::WorldSide;
    use super::store::{ReplicaWorld, ServerWorld, World};

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

    pub fn flat_server_world() -> ServerWorld {
        let mut w = ServerWorld::new(0, 1);
        install_flat_floor(&mut w);
        w
    }

    pub fn flat_replica_world() -> ReplicaWorld {
        let mut w = ReplicaWorld::new(0, 1);
        install_flat_floor(&mut w);
        w
    }
}
