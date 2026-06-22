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
pub mod chunk;
pub mod controls;
pub mod entity;
pub mod game;
pub mod inventory;
pub mod item;
pub mod mathh;
pub mod mesh;
pub mod mining;
pub mod platform;
pub mod player;
pub mod render;
pub mod save;
pub mod worker;
pub mod world;
pub mod worldgen;

pub use atlas::Tile;
