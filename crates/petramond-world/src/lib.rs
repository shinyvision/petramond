//! The deterministic world core: content registries and catalogs, the
//! block/item domain, section/column storage, and the data half of the world
//! (`world::WorldData`). No GPU, no audio, no networking, no WASM — the
//! engine crate layers orchestration on top.
//!
//! Client and persistence code that only consumes this crate lives in its own
//! crates so everything depending on the core (worldgen, mesh) does not link
//! it: input bindings in `petramond-input`, the animator graph runtime in
//! `petramond-anim`, the music catalog in `petramond-audio`, the region-file
//! container in `petramond-region`, and view-volume culling math in
//! `petramond-math`.

#![allow(clippy::too_many_arguments)]

// Foundation aliases so module-internal `crate::mathh`-style paths resolve
// unchanged after extraction from the monolith.
pub use petramond_math::{face, facing, math as mathh, wire_enum};
pub use petramond_util::{memory, paths, test_time, texture_mips};

pub mod ai_vocab;
pub mod animated_model;
pub mod asset_cache;
pub mod assets;
pub mod bbmodel;
pub mod biome;
pub mod block;
pub mod block_model;
pub mod block_state;
pub mod body;
pub mod border;
pub mod chunk;
pub mod collision;
pub mod column;
#[cfg(any(test, feature = "test-support"))]
pub mod column_split;
pub mod condition;
pub mod connect;
pub mod construction;
pub mod container;
pub mod crafting;
pub mod damage;
pub mod door;
pub mod effect;
pub mod exposure;
pub mod fence;
pub mod fluid;
pub mod fluid_math;
pub mod furnace;
pub mod gui_state;
pub mod inventory;
pub mod item;
pub mod ladder;
pub mod light;
pub mod loot;
pub mod mining;
pub mod pack_manifest;
pub mod pane;
pub mod particle_emitters;
pub mod registry;
pub mod section;
pub mod shade;
pub mod shape_mesh;
pub mod slab;
pub mod sound_registry;
pub mod stair;
pub mod structure;
#[cfg(any(test, feature = "test-support"))]
pub mod test_child;
pub mod texture_transition;
pub mod tile;
pub mod tile_alpha;
pub mod torch;
pub mod trapdoor;
pub mod world;
