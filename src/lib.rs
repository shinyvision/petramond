//! Llamacraft: noise-driven voxel world with biomes, trees, rivers.
//!
//! Native desktop target. Worldgen runs off the render thread via an OS thread
//! pool (rayon).

#![allow(clippy::too_many_arguments)]

pub mod app;
pub mod atlas;
pub mod biome;
pub mod block;
pub mod camera;
pub mod chest;
pub mod chunk;
pub mod controls;
pub mod crafting;
pub mod entity;
pub mod furnace;
pub mod game;
pub mod inventory;
pub mod item;
pub mod mathh;
pub mod mesh;
pub mod mining;
pub mod mob;
pub mod platform;
pub mod player;
pub mod registry;
pub mod render;
pub mod save;
pub mod torch;
pub mod worker;
pub mod world;
pub mod worldgen;

pub use atlas::Tile;
