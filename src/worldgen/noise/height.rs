//! Cave field and active cave carving helpers.
//!
//! Cave decisions are plain typed functions of world position plus the column's
//! original density surface, so caves are identical from every chunk/section that
//! touches them: seamless tunnels and entrances with no inter-chunk state.

use super::settings::*;

use crate::block::Block;
use crate::chunk::{idx, section_idx, Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SECTION_SIZE};
use crate::section::Section;
use noise::{NoiseFn, OpenSimplex};

/// Owns the cave noise samplers and decides whether a solid voxel is carved to
/// air. Immutable after construction; `Send + Sync`.
///
/// Each sampler is salt-seeded (`OpenSimplex::new(seed.wrapping_add(SALT_CAVE_*))`)
/// so construction order is irrelevant and output is a pure function of seed.
pub struct CaveField {
    cave_a: OpenSimplex, // spaghetti tunnel field A
    cave_b: OpenSimplex, // spaghetti tunnel field B
    cave_c: OpenSimplex, // cheese cavern field
    roughness: OpenSimplex,
    entrance_a: OpenSimplex,
    entrance_b: OpenSimplex,
}

impl CaveField {
    pub fn new(seed: u32) -> Self {
        let s = |salt: u32| seed.wrapping_add(salt);
        Self {
            cave_a: OpenSimplex::new(s(SALT_CAVE_A)),
            cave_b: OpenSimplex::new(s(SALT_CAVE_B)),
            cave_c: OpenSimplex::new(s(SALT_CAVE_C)),
            roughness: OpenSimplex::new(s(SALT_CAVE_ROUGHNESS)),
            entrance_a: OpenSimplex::new(s(SALT_CAVE_ENTRANCE_A)),
            entrance_b: OpenSimplex::new(s(SALT_CAVE_ENTRANCE_B)),
        }
    }

    /// Should the solid voxel at world `(x,y,z)` be carved to air? `surf_y` is the
    /// original density top-solid surface for the voxel's `(x,z)` column.
    #[inline]
    pub fn cave_carved(&self, x: i32, y: i32, z: i32, surf_y: i32) -> bool {
        if y > surf_y {
            return false;
        }
        if self.entrance_carved(x, y, z, surf_y) {
            return true;
        }
        if y < CAVE_MIN_Y || y > surf_y - CAVE_SURFACE_BUFFER {
            return false;
        }

        // Spaghetti: both decorrelated fields near zero -> a winding tunnel.
        let spaghetti = self.spaghetti_sample(x, y, z);
        if spaghetti.metric < spaghetti.tunnel_r {
            return true;
        }

        // Cheese: a low-frequency field dipping low -> occasional large caverns.
        let (fx, fy, fz) = (x as f64, y as f64, z as f64);
        let cheese = self.cave_c.get([
            fx * CAVE_CHEESE_FREQ,
            fy * CAVE_CHEESE_FREQ * 1.4,
            fz * CAVE_CHEESE_FREQ,
        ]);
        cheese < CAVE_CHEESE_T + spaghetti.rough * CAVE_CHEESE_ROUGHNESS
    }

    /// Post-cave top non-air surface for a land column, before vegetation/trees.
    ///
    /// Most columns return `surf_y` without scanning. Only when the entrance field
    /// actually cuts the surface do we walk down until the first non-carved voxel,
    /// matching the later block carve.
    pub fn surface_after_caves(&self, x: i32, z: i32, surf_y: i32) -> i32 {
        if !self.entrance_carved(x, surf_y, z, surf_y) {
            return surf_y;
        }
        let mut y = surf_y;
        while y >= CAVE_MIN_Y && self.cave_carved(x, y, z, surf_y) {
            y -= 1;
        }
        y
    }

    /// Surface used only for tree/feature anchoring. Cave-mouth columns are
    /// deliberately treated as unsuitable roots so generated trunks do not plug
    /// entrances.
    pub fn feature_surface_after_caves(&self, x: i32, z: i32, surf_y: i32) -> i32 {
        let top = self.surface_after_caves(x, z, surf_y);
        if top < surf_y {
            CAVE_ENTRANCE_MIN_SURFACE_Y
                .min(surf_y)
                .min(crate::chunk::SEA_LEVEL)
        } else {
            surf_y
        }
    }

    /// Conservative generated-summary helper. If this returns true the section may
    /// contain cave air, so callers must not claim it is virtual full stone.
    pub fn section_may_carve(cy: i32, surf_min: i32, surf_max: i32) -> bool {
        let y0 = cy * SECTION_SIZE as i32;
        let y1 = y0 + SECTION_SIZE as i32 - 1;
        if y0 > surf_max || y1 < CAVE_MIN_Y {
            return false;
        }

        let interior = y0 <= surf_max - CAVE_SURFACE_BUFFER;
        let entrance = surf_max >= CAVE_ENTRANCE_MIN_SURFACE_Y
            && y0 <= surf_max
            && y1 >= surf_min - CAVE_ENTRANCE_MAX_DEPTH;
        interior || entrance
    }

    pub fn carve_chunk(&self, chunk: &mut Chunk, surf: &[i32]) {
        debug_assert_eq!(surf.len(), CHUNK_SX * CHUNK_SZ);
        let (ox, oz) = chunk.chunk_origin_world();
        let air = Block::Air.id();
        let water = Block::Water.id();
        let mut carved = false;
        let blocks = chunk.blocks_slice_mut();

        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                let surf_y = surf[z * CHUNK_SX + x];
                let y0 = CAVE_MIN_Y.max(0);
                let y1 = surf_y.min(CHUNK_SY as i32 - 1);
                if y0 > y1 {
                    continue;
                }
                let wx = ox + x as i32;
                let wz = oz + z as i32;
                for y in y0..=y1 {
                    let i = idx(x, y as usize, z);
                    let id = blocks[i];
                    if id == air || id == water {
                        continue;
                    }
                    if self.cave_carved(wx, y, wz, surf_y) {
                        blocks[i] = air;
                        carved = true;
                    }
                }
            }
        }

        if carved {
            chunk.recompute_heightmap();
            chunk.recompute_random_tick_count();
        }
    }

    pub fn carve_section(&self, section: &mut Section, surf: &[i32]) {
        debug_assert_eq!(surf.len(), SECTION_SIZE * SECTION_SIZE);
        let (ox, oy, oz) = section.origin_world();
        let air = Block::Air.id();
        let water = Block::Water.id();
        let blocks = section.blocks_slice_mut();

        for z in 0..SECTION_SIZE {
            for x in 0..SECTION_SIZE {
                let surf_y = surf[z * SECTION_SIZE + x];
                let y0 = oy.max(CAVE_MIN_Y);
                let y1 = (oy + SECTION_SIZE as i32 - 1).min(surf_y);
                if y0 > y1 {
                    continue;
                }
                let wx = ox + x as i32;
                let wz = oz + z as i32;
                for wy in y0..=y1 {
                    let ly = (wy - oy) as usize;
                    let i = section_idx(x, ly, z);
                    let id = blocks[i];
                    if id == air || id == water {
                        continue;
                    }
                    if self.cave_carved(wx, wy, wz, surf_y) {
                        blocks[i] = air;
                    }
                }
            }
        }
    }

    #[inline]
    fn entrance_carved(&self, x: i32, y: i32, z: i32, surf_y: i32) -> bool {
        if surf_y < CAVE_ENTRANCE_MIN_SURFACE_Y {
            return false;
        }
        let depth = surf_y - y;
        if !(0..=CAVE_ENTRANCE_MAX_DEPTH).contains(&depth) {
            return false;
        }

        let t = depth as f64 / CAVE_ENTRANCE_MAX_DEPTH as f64;
        let ease = smoothstep(t);
        let threshold = lerp(
            CAVE_ENTRANCE_GATE_SURFACE_T,
            CAVE_ENTRANCE_GATE_DEEP_T,
            ease,
        );

        let (fx, fy, fz) = (x as f64, y as f64, z as f64);
        let gate = self.entrance_a.get([
            fx * CAVE_ENTRANCE_FREQ,
            fy * CAVE_ENTRANCE_FREQ * CAVE_ENTRANCE_Y_SCALE,
            fz * CAVE_ENTRANCE_FREQ,
        ]) + 0.35
            * self.entrance_b.get([
                fx * CAVE_ENTRANCE_FREQ * 1.7 + 37.1,
                fy * CAVE_ENTRANCE_FREQ * CAVE_ENTRANCE_Y_SCALE * 1.3 + 11.3,
                fz * CAVE_ENTRANCE_FREQ * 1.7 - 19.7,
            ]);
        if gate > threshold {
            return false;
        }

        let spaghetti = self.spaghetti_sample(x, y, z);
        let base_r = lerp(CAVE_ENTRANCE_SURFACE_R, CAVE_ENTRANCE_DEEP_R, ease);
        let radius = (base_r + spaghetti.rough * CAVE_TUNNEL_ROUGHNESS).max(0.030);
        spaghetti.metric < radius
    }

    #[inline]
    fn spaghetti_sample(&self, x: i32, y: i32, z: i32) -> SpaghettiSample {
        let (fx, fy, fz) = (x as f64, y as f64, z as f64);
        let rough = self.roughness.get([
            fx * CAVE_ROUGHNESS_FREQ,
            fy * CAVE_ROUGHNESS_FREQ * 0.7,
            fz * CAVE_ROUGHNESS_FREQ,
        ]);
        let tunnel_r = (CAVE_TUNNEL_R + rough * CAVE_TUNNEL_ROUGHNESS).max(0.035);
        let a = self
            .cave_a
            .get([fx * CAVE_FREQ_XZ, fy * CAVE_FREQ_Y, fz * CAVE_FREQ_XZ]);
        let b = self.cave_b.get([
            fx * CAVE_FREQ_XZ + 13.7,
            fy * CAVE_FREQ_Y + 5.1,
            fz * CAVE_FREQ_XZ - 7.3,
        ]);

        SpaghettiSample {
            metric: a.abs().max(b.abs()),
            tunnel_r,
            rough,
        }
    }
}

struct SpaghettiSample {
    metric: f64,
    tunnel_r: f64,
    rough: f64,
}

#[inline]
fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

#[inline]
fn smoothstep(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
