//! `noise` subsystem — the `WorldNoise` facade over a typed `HeightField`.
//!
//! Strata P1: all height/climate/river math now lives in `height::HeightField`,
//! configured by the const table in `settings`. `WorldNoise` is a thin,
//! ABI-preserving facade kept for `app.rs` (`WorldNoise::new(seed).climate(..)`)
//! and the `crate::gen` shim. It owns the field and delegates; it holds no
//! interior mutability and is plain `Send + Sync`.

pub mod height;
pub mod settings;

pub use height::HeightField;

use crate::biome::Climate;

pub struct WorldNoise {
    pub seed: u32,
    field: HeightField,
}

impl WorldNoise {
    pub fn new(seed: u32) -> Self {
        Self {
            seed,
            field: HeightField::new(seed),
        }
    }

    #[inline]
    pub fn climate(&self, x: i32, z: i32) -> Climate {
        self.field.climate(x, z)
    }

    #[inline]
    pub fn surface_height(&self, x: i32, z: i32) -> i32 {
        self.field.surface_height(x, z)
    }

    #[inline]
    pub fn river_strength(&self, x: i32, z: i32) -> f32 {
        self.field.river_strength(x, z)
    }

    /// Debug: raw noise field samples (cont, erosion, pv, jagged) at a column.
    pub fn debug_sample(&self, x: i32, z: i32) -> (f64, f64, f64, f64) {
        self.field.debug_sample(x, z)
    }

    /// Debug: raw weirdness sample at a column.
    pub fn debug_weirdness(&self, x: i32, z: i32) -> f64 {
        self.field.debug_weirdness(x, z)
    }
}
