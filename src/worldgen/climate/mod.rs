//! Climate types shared by the legacy native sampler path (now used only by the
//! genmap diagnostic modes, not the live cascade generator).
//!
//! `Climate` itself stays in `crate::biome` (its 6 fields are part of the
//! sampler ABI); it is re-exported here for convenience.

pub use crate::biome::Climate;
