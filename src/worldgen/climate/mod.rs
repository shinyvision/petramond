//! Climate → biome selection.
//!
//! `Climate` itself stays in `crate::biome` (its 6 fields are part of the
//! sampler ABI); it is re-exported here for convenience. `source` defines the
//! `BiomeSource` trait and the parity-preserving `CascadeBiomeSource`.

pub mod source;

pub use crate::biome::Climate;
