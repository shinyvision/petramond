//! Petramond: noise-driven voxel world with biomes and trees.
//!
//! Native desktop target. Worldgen runs off the render thread via an OS thread
//! pool (rayon).

#![allow(clippy::too_many_arguments)]

pub mod entity;
pub mod events;
pub mod gui;
pub mod menu;
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
