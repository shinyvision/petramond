//! Petramond: noise-driven voxel world with biomes and trees.
//!
//! Native desktop target. Worldgen runs off the render thread via an OS thread
//! pool (rayon).

#![allow(clippy::too_many_arguments)]

pub mod entity;
pub mod events;
// Transitional aliases while the core is decomposed into workspace crates:
// extracted modules stay addressable under their old `crate::` paths until the
// facade dissolves.
pub use petramond_math::{face, facing, math as mathh, wire_enum};
// The deterministic world core, re-exported under its historical module paths.
pub use petramond_world::{
    ai_vocab, block_model, shade, shape_mesh,
    asset_cache, assets, bbmodel, biome, block, block_state, body, chunk, collision, column,
    connect, container, controls, crafting, damage, door, effect, fence, furnace, gui_state,
    inventory, item, keycode, ladder, light, mining, pack_manifest, pane, particle_emitters,
    registry, section, slab, sound_registry, stair, tile, tile_alpha, torch, view_volume,
};
pub use petramond_util::{memory, texture_mips};
pub mod gui;
pub mod menu;
pub use petramond_mesh as mesh;
pub mod mob;
pub mod modding;
pub mod net;
pub mod platform;
pub mod player;
pub mod save;
pub mod server;
pub mod tooling;
pub mod worker;
pub mod world;
pub use petramond_worldgen as worldgen;

pub use petramond_util::test_time;
