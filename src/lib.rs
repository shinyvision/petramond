//! Llamacraft: noise-driven voxel world with biomes and trees.
//!
//! Native desktop target. Worldgen runs off the render thread via an OS thread
//! pool (rayon).

#![allow(clippy::too_many_arguments)]

/// The gen/light/mesh worker pools allocate large short-lived buffers from many
/// threads at once; mimalloc's per-thread heaps keep that churn off the system
/// allocator's shared arena locks (measured as residual frame-time spikes).
#[global_allocator]
static GLOBAL_ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod app;
mod asset_cache;
mod atlas;
mod audio;
mod bbmodel;
mod biome;
mod block;
mod block_model;
mod camera;
mod chest;
mod chunk;
mod collision;
mod column;
mod controls;
mod crafting;
mod door;
mod entity;
mod furnace;
mod game;
mod gui;
mod inventory;
mod item;
mod mathh;
mod mesh;
mod mining;
mod mob;
pub mod platform;
mod player;
mod render;
mod save;
mod section;
mod stair;
mod texture_mips;
pub mod tooling;
mod torch;
mod worker;
mod world;
mod worldgen;
