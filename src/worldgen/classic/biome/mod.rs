//! The layered biome cascade — produces a biome id at every world column as a
//! pure function of `(world_seed, x, z)`, bit-exact to the reference ruleset.
//!
//! The stack starts coarse (1 cell = 4096 blocks) and zooms to 1:1, applying
//! land/ocean shaping, climate, biome selection, edges, hills, shores, and a
//! river overlay, then a final per-block jitter. Each layer is a pure transform of
//! its parent grid driven by the 64-bit layer generator ([`super::layer_rng`]).
//!
//! Layers are ported and verified one at a time against the reference's
//! per-layer output; see [`layers`].

pub mod ids;
pub mod layers;
pub mod stack;
