//! Llamacraft: noise-driven voxel world with biomes, trees, rivers.
//!
//! Native + wasm32 (web) targets. Worldgen runs off the render thread:
//! native = OS thread pool (rayon), web = dedicated `Worker`.

#![allow(clippy::too_many_arguments)]

pub mod atlas;
pub mod app;
pub mod block;
pub mod biome;
pub mod camera;
pub mod chunk;
pub mod mathh;
pub mod worldgen;
pub mod mesh;
pub mod player;
pub mod platform;
pub mod render;
pub mod world;
pub mod worker;

pub use atlas::Tile;