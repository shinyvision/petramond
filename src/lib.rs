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
