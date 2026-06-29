//! Cave field.
//!
//! The 3-D cave carve is a plain typed function of world position, so caves are
//! identical from every chunk that touches them: seamless tunnels with no
//! inter-chunk state.

use super::settings::*;

use noise::{NoiseFn, OpenSimplex};

/// Owns the three cave noise samplers and decides whether a solid voxel is carved
/// to air. Immutable after construction; `Send + Sync`.
///
/// Each sampler is salt-seeded (`OpenSimplex::new(seed.wrapping_add(SALT_CAVE_*))`)
/// so construction order is irrelevant and output is a pure function of seed.
pub struct CaveField {
    cave_a: OpenSimplex, // spaghetti tunnel field A
    cave_b: OpenSimplex, // spaghetti tunnel field B
    cave_c: OpenSimplex, // cheese cavern field
}

impl CaveField {
    pub fn new(seed: u32) -> Self {
        let s = |salt: u32| seed.wrapping_add(salt);
        Self {
            cave_a: OpenSimplex::new(s(SALT_CAVE_A)),
            cave_b: OpenSimplex::new(s(SALT_CAVE_B)),
            cave_c: OpenSimplex::new(s(SALT_CAVE_C)),
        }
    }

    /// Should the solid voxel at world `(x, y, z)` be carved to air (a cave)?
    /// Pure function of world position, so caves are identical from every chunk
    /// that touches them — seamless tunnels with no inter-chunk state. The caller
    /// restricts the Y band (keep a floor + solid rock under the surface skin).
    #[inline]
    pub fn cave_carved(&self, x: i32, y: i32, z: i32) -> bool {
        let (fx, fy, fz) = (x as f64, y as f64, z as f64);
        // Spaghetti: both decorrelated fields near zero -> a winding tunnel.
        let a = self
            .cave_a
            .get([fx * CAVE_FREQ_XZ, fy * CAVE_FREQ_Y, fz * CAVE_FREQ_XZ]);
        if a.abs() < CAVE_TUNNEL_R {
            let b = self.cave_b.get([
                fx * CAVE_FREQ_XZ + 13.7,
                fy * CAVE_FREQ_Y + 5.1,
                fz * CAVE_FREQ_XZ - 7.3,
            ]);
            if b.abs() < CAVE_TUNNEL_R {
                return true;
            }
        }
        // Cheese: a low-frequency field dipping low -> occasional large caverns.
        let cheese = self.cave_c.get([
            fx * CAVE_CHEESE_FREQ,
            fy * CAVE_CHEESE_FREQ * 1.4,
            fz * CAVE_CHEESE_FREQ,
        ]);
        cheese < CAVE_CHEESE_T
    }
}
