//! Placement modifiers — declarative where / how-often / when for a feature.
//!
//! Defined now so `PlacedFeature` rows can be authored; the generalized
//! modifier-walking driver is activated in P4. Through P3 the placement loop
//! reproduces `tree_probability` / `pick_oak_variant` exactly instead, so these
//! variants are not yet evaluated at runtime.

use crate::biome::Biome;

pub enum PlacementModifier {
    /// 1-in-N-ish per-column probability (replaces `tree_probability`).
    Rarity(f32),
    /// {min, max} placement attempts per chunk.
    CountPerChunk(u8, u8),
    /// Anchor the feature's Y to the column surface height.
    HeightmapAnchor,
    /// Require the column surface to be above sea level.
    AboveSeaLevel,
    /// Restrict to the listed biomes.
    BiomeFilter(&'static [Biome]),
    /// Jitter the placement within the column cell.
    InSquare,
}
