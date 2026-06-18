//! `SurfaceSystem` — composes the column's surface material.
//!
//! Strata P2: a global river/beach pre-pass (cross-cutting, applies in every
//! biome) wraps each biome's declarative top rule; the subsurface material is a
//! per-biome block. Together these reproduce `surface_block`/`subsurface_block`
//! exactly. P4 generalises this into a single layered per-cell `SurfaceRule`
//! stack per biome.

pub mod rule;

use crate::biome::Biome;
use crate::block::Block;
use crate::chunk::SEA_LEVEL;
use rule::SurfaceCtx;

use super::data::biomes::def;

pub struct SurfaceSystem;

impl SurfaceSystem {
    /// Top (surface) block at a column. Mirrors `surface_block(biome, surf, river)`:
    /// the river/beach sand pre-pass first (using y = surf), then the biome's
    /// declarative top rule (which carries the mountain altitude bands).
    pub fn top_block(&self, biome: Biome, surf: i32, river: f32) -> Block {
        if river > 0.05 && surf <= SEA_LEVEL + 1 {
            return Block::Sand;
        }
        let ctx = SurfaceCtx {
            y: surf,
            surf_y: surf,
            depth_from_top: 0,
            biome,
            river,
        };
        def(biome).surface_top.resolve(&ctx).unwrap_or(Block::Stone)
    }

    /// Subsurface block (the band just below the surface). Mirrors
    /// `subsurface_block(biome)` — altitude-independent, no river override.
    #[inline]
    pub fn subsurface(&self, biome: Biome) -> Block {
        def(biome).subsurface
    }
}
