//! Llamacraft: noise-driven voxel world with biomes and trees.
//!
//! Native desktop target. Worldgen runs off the render thread via an OS thread
//! pool (rayon).

#![allow(clippy::too_many_arguments)]

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
// Cubic-chunks refactor (in progress): `Section`/`Column` are built additively and
// wired into `World` in a later stage; allow dead code until the flip.
mod collision;
#[allow(dead_code)]
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
pub mod perf;
pub mod platform;
mod player;
mod registry;
mod render;
mod save;
#[allow(dead_code)]
mod section;
mod texture_mips;
pub mod tooling;
mod torch;
mod worker;
mod world;
mod worldgen;
