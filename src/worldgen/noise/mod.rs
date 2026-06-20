//! `noise` subsystem — the `WorldNoise` facade over a typed `HeightField`.
//!
//! This is retained for legacy diagnostics and cave sampling. Active chunk
//! terrain uses the classic biome terrain provider plus `worldgen::river` for
//! explicit river carving.

pub mod height;
pub mod settings;
pub mod spline;

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

    /// Debug: raw noise field samples (cont, erosion, pv, jagged) at a column.
    pub fn debug_sample(&self, x: i32, z: i32) -> (f64, f64, f64, f64) {
        self.field.debug_sample(x, z)
    }

    /// Debug: raw weirdness sample at a column.
    pub fn debug_weirdness(&self, x: i32, z: i32) -> f64 {
        self.field.debug_weirdness(x, z)
    }

    /// Debug: landform weights (mountain, foothill, rolling, plateau, wet_basin).
    pub fn debug_landform(&self, x: i32, z: i32) -> (f32, f32, f32, f32, f32) {
        self.field.debug_landform(x, z)
    }
}
