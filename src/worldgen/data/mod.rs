//! Worldgen content tables — the single authoring surface.
//!
//! Strata P2: `biomes` holds one `BiomeDef` per biome (surface composition).
//! P3 adds `features` (configured/placed feature rows); P4 folds tints and
//! climate-placement points in here too, so a biome is defined in exactly one
//! place.

pub mod biomes;
pub mod features;
pub mod rivers;
