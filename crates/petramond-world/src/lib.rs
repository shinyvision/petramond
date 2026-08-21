//! The deterministic world core: content registries and catalogs, the
//! block/item domain, section/column storage, and the data half of the world
//! (`world::WorldData`). No GPU, no audio, no networking, no WASM — the
//! engine crate layers orchestration on top.

#![allow(clippy::too_many_arguments)]

// Foundation aliases so module-internal `crate::mathh`-style paths resolve
// unchanged after extraction from the monolith.
pub use petramond_math::{face, facing, math as mathh, wire_enum};
pub use petramond_util::{memory, paths, test_time, texture_mips};

pub mod ai_vocab;
pub mod asset_cache;
pub mod assets;
pub mod bbmodel;
pub mod biome;
pub mod block;
pub mod block_model;
pub mod block_state;
pub mod body;
pub mod chunk;
pub mod collision;
pub mod column;
#[cfg(any(test, feature = "test-support"))]
pub mod column_split;
pub mod connect;
pub mod container;
pub mod controls;
pub mod crafting;
pub mod damage;
pub mod door;
pub mod effect;
pub mod fence;
pub mod furnace;
pub mod gui_state;
pub mod inventory;
pub mod item;
pub mod keycode;
pub mod ladder;
pub mod light;
pub mod mining;
pub mod pack_manifest;
pub mod pane;
pub mod particle_emitters;
pub mod region;
pub mod registry;
pub mod section;
pub mod shade;
pub mod shape_mesh;
pub mod slab;
pub mod sound_registry;
pub mod stair;
pub mod tile;
pub mod tile_alpha;
pub mod torch;
pub mod view_volume;
pub mod water_math;
pub mod world;
