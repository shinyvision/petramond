//! The data half of the world: [`data::WorldData`] plus the query/state
//! modules that operate purely on it. The orchestration wrapper (`World`)
//! lives in the engine crate and exposes [`data::WorldData`] read-only.

pub mod column_heightmaps;
pub mod cursor;
pub mod custom_bake;
pub mod data;
pub mod environment;
pub mod fence;
pub mod kv;
pub mod ladder;
pub mod light;
pub mod load_targets;
pub mod model;
pub mod neighborhood;
pub mod pane;
pub mod placement;
pub mod query;
pub mod raycast;
pub mod saved_index;
pub mod shape_bake_validate;
pub mod slab;
pub mod stair;
#[cfg(test)]
pub(crate) mod test_world;
pub mod tick_state;
pub mod torch;

pub use cursor::SectionCursor;
pub use data::WorldData;
pub use saved_index::SavedIndex;
