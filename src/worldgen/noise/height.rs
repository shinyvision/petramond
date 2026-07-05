//! Cave field and active cave carving helpers.
//!
//! Cave decisions are plain typed functions of world position plus the column's
//! original density surface, so caves are identical from every chunk/section that
//! touches them: seamless tunnels and entrances with no inter-chunk state.
//!
//! The four interior fields (spaghetti A/B, roughness, cheese) are sampled on a
//! world-anchored [`LATTICE_STEP`]-block lattice and trilinearly interpolated per
//! voxel — the highest field frequency (roughness, 0.034 ≈ 29-block wavelength)
//! is far below the lattice's Nyquist limit, and per-voxel OpenSimplex sampling
//! was ~a quarter of all worldgen CPU. Anchoring lattice points to absolute
//! multiples of `LATTICE_STEP` makes every path — per-section carve, whole-chunk
//! carve, per-point surface walks — read identical values, so caves stay seamless
//! and column heightmaps stay consistent with carved blocks. The entrance GATE
//! fields stay exactly-sampled: they are evaluated once per column (hot in
//! `ColumnGen`) where a lazy point lattice would cost more than the two samples
//! they replace.

use super::settings::*;

use crate::block::Block;
use crate::chunk::{idx, section_idx, Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SECTION_SIZE};
use crate::section::Section;
use noise::{NoiseFn, OpenSimplex};

/// Interior-field lattice spacing in blocks. Section (16) and chunk origins are
/// multiples of this, so batch lattices land exactly on section corners.
const LATTICE_STEP: i32 = 4;
const LATTICE_STEP_F: f64 = LATTICE_STEP as f64;

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

/// The interior cave fields of one axis-aligned box, sampled at every world
/// lattice point the box touches. Built per carve batch (section / chunk) or,
/// degenerately, per point for the rare surface walks.
struct CaveLattice {
    lx0: i32,
    ly0: i32,
    lz0: i32,
    nx: usize,
    ny: usize,
    nz: usize,
    a: Vec<f64>,
    b: Vec<f64>,
    rough: Vec<f64>,
    cheese: Vec<f64>,
}

impl CaveLattice {
    /// Trilinear interpolation of `field` at world voxel `(x,y,z)` (must lie inside
    /// the box the lattice was built for).
    #[inline]
    fn tri(&self, field: &[f64], x: i32, y: i32, z: i32) -> f64 {
        let cx = (x.div_euclid(LATTICE_STEP) - self.lx0) as usize;
        let cy = (y.div_euclid(LATTICE_STEP) - self.ly0) as usize;
        let cz = (z.div_euclid(LATTICE_STEP) - self.lz0) as usize;
        let tx = x.rem_euclid(LATTICE_STEP) as f64 / LATTICE_STEP_F;
        let ty = y.rem_euclid(LATTICE_STEP) as f64 / LATTICE_STEP_F;
        let tz = z.rem_euclid(LATTICE_STEP) as f64 / LATTICE_STEP_F;
        debug_assert!(cx + 1 < self.nx && cy + 1 < self.ny && cz + 1 < self.nz);

        let i =
            |dx: usize, dy: usize, dz: usize| ((cy + dy) * self.nz + cz + dz) * self.nx + cx + dx;
        let lerp = |a: f64, b: f64, t: f64| a + (b - a) * t;
        let x00 = lerp(field[i(0, 0, 0)], field[i(1, 0, 0)], tx);
        let x01 = lerp(field[i(0, 0, 1)], field[i(1, 0, 1)], tx);
        let x10 = lerp(field[i(0, 1, 0)], field[i(1, 1, 0)], tx);
        let x11 = lerp(field[i(0, 1, 1)], field[i(1, 1, 1)], tx);
        let z0 = lerp(x00, x01, tz);
        let z1 = lerp(x10, x11, tz);
        lerp(z0, z1, ty)
    }

    #[inline]
    fn a(&self, x: i32, y: i32, z: i32) -> f64 {
        self.tri(&self.a, x, y, z)
    }
    #[inline]
    fn b(&self, x: i32, y: i32, z: i32) -> f64 {
        self.tri(&self.b, x, y, z)
    }
    #[inline]
    fn rough(&self, x: i32, y: i32, z: i32) -> f64 {
        self.tri(&self.rough, x, y, z)
    }
    #[inline]
    fn cheese(&self, x: i32, y: i32, z: i32) -> f64 {
        self.tri(&self.cheese, x, y, z)
    }
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

    // Raw field samplers — the ONE place each field's frequency/offset math lives,
    // so lattice corners and any exact query can never drift apart.
    fn sample_a(&self, x: f64, y: f64, z: f64) -> f64 {
        self.cave_a
            .get([x * CAVE_FREQ_XZ, y * CAVE_FREQ_Y, z * CAVE_FREQ_XZ])
    }
    fn sample_b(&self, x: f64, y: f64, z: f64) -> f64 {
        self.cave_b.get([
            x * CAVE_FREQ_XZ + 13.7,
            y * CAVE_FREQ_Y + 5.1,
            z * CAVE_FREQ_XZ - 7.3,
        ])
    }
    fn sample_rough(&self, x: f64, y: f64, z: f64) -> f64 {
        self.roughness.get([
            x * CAVE_ROUGHNESS_FREQ,
            y * CAVE_ROUGHNESS_FREQ * 0.7,
            z * CAVE_ROUGHNESS_FREQ,
        ])
    }
    fn sample_cheese(&self, x: f64, y: f64, z: f64) -> f64 {
        self.cave_c.get([
            x * CAVE_CHEESE_FREQ,
            y * CAVE_CHEESE_FREQ * 1.4,
            z * CAVE_CHEESE_FREQ,
        ])
    }

    /// Sample the interior fields at every world lattice point covering the inclusive
    /// voxel box `(x0..=x1, y0..=y1, z0..=z1)`.
    fn build_lattice(&self, x0: i32, y0: i32, z0: i32, x1: i32, y1: i32, z1: i32) -> CaveLattice {
        let lx0 = x0.div_euclid(LATTICE_STEP);
        let ly0 = y0.div_euclid(LATTICE_STEP);
        let lz0 = z0.div_euclid(LATTICE_STEP);
        let nx = (x1.div_euclid(LATTICE_STEP) + 1 - lx0) as usize + 1;
        let ny = (y1.div_euclid(LATTICE_STEP) + 1 - ly0) as usize + 1;
        let nz = (z1.div_euclid(LATTICE_STEP) + 1 - lz0) as usize + 1;
        let n = nx * ny * nz;
        let mut lat = CaveLattice {
            lx0,
            ly0,
            lz0,
            nx,
            ny,
            nz,
            a: Vec::with_capacity(n),
            b: Vec::with_capacity(n),
            rough: Vec::with_capacity(n),
            cheese: Vec::with_capacity(n),
        };
        for ly in 0..ny {
            let fy = ((ly0 + ly as i32) * LATTICE_STEP) as f64;
            for lz in 0..nz {
                let fz = ((lz0 + lz as i32) * LATTICE_STEP) as f64;
                for lx in 0..nx {
                    let fx = ((lx0 + lx as i32) * LATTICE_STEP) as f64;
                    lat.a.push(self.sample_a(fx, fy, fz));
                    lat.b.push(self.sample_b(fx, fy, fz));
                    lat.rough.push(self.sample_rough(fx, fy, fz));
                    lat.cheese.push(self.sample_cheese(fx, fy, fz));
                }
            }
        }
        lat
    }

    /// Should the solid voxel at world `(x,y,z)` be carved to air? `surf_y` is the
    /// original density top-solid surface for the voxel's `(x,z)` column.
    ///
    /// Point-query form: gates first (exact, cheap), then a degenerate one-voxel
    /// lattice — the SAME evaluator as the batch carve, so surface walks always agree
    /// with carved blocks. Only for sparse queries; batches use [`carve_section`] /
    /// [`carve_chunk`], which amortize one lattice over the whole box.
    pub fn cave_carved(&self, x: i32, y: i32, z: i32, surf_y: i32) -> bool {
        if y > surf_y {
            return false;
        }
        let gate = self.entrance_gate_ease(x, y, z, surf_y);
        let interior = y >= CAVE_MIN_Y && y <= surf_y - CAVE_SURFACE_BUFFER;
        if gate.is_none() && !interior {
            return false;
        }
        let lat = self.build_lattice(x, y, z, x, y, z);
        self.carved_from_lattice(&lat, x, y, z, surf_y, gate, interior)
    }

    /// The one carve decision, given interpolated interior fields. `gate` and
    /// `interior` are precomputed by the caller (the point path uses them to skip
    /// building a lattice at all for the common solid voxel).
    #[inline]
    fn carved_from_lattice(
        &self,
        lat: &CaveLattice,
        x: i32,
        y: i32,
        z: i32,
        surf_y: i32,
        gate: Option<f64>,
        interior: bool,
    ) -> bool {
        debug_assert!(y <= surf_y);
        if let Some(ease) = gate {
            let rough = lat.rough(x, y, z);
            let metric = lat.a(x, y, z).abs().max(lat.b(x, y, z).abs());
            let base_r = lerp(CAVE_ENTRANCE_SURFACE_R, CAVE_ENTRANCE_DEEP_R, ease);
            let radius = (base_r + rough * CAVE_TUNNEL_ROUGHNESS).max(0.030);
            if metric < radius {
                return true;
            }
        }
        if !interior {
            return false;
        }

        // Spaghetti: both decorrelated fields near zero -> a winding tunnel.
        let rough = lat.rough(x, y, z);
        let metric = lat.a(x, y, z).abs().max(lat.b(x, y, z).abs());
        let tunnel_r = (CAVE_TUNNEL_R + rough * CAVE_TUNNEL_ROUGHNESS).max(0.035);
        if metric < tunnel_r {
            return true;
        }

        // Cheese: a low-frequency field dipping low -> occasional large caverns.
        lat.cheese(x, y, z) < CAVE_CHEESE_T + rough * CAVE_CHEESE_ROUGHNESS
    }

    #[inline]
    fn carved_lat(&self, lat: &CaveLattice, x: i32, y: i32, z: i32, surf_y: i32) -> bool {
        if y > surf_y {
            return false;
        }
        let gate = self.entrance_gate_ease(x, y, z, surf_y);
        let interior = y >= CAVE_MIN_Y && y <= surf_y - CAVE_SURFACE_BUFFER;
        if gate.is_none() && !interior {
            return false;
        }
        self.carved_from_lattice(lat, x, y, z, surf_y, gate, interior)
    }

    /// Post-cave top non-air surface for a land column, before vegetation/trees.
    ///
    /// Most columns return `surf_y` without scanning. Only when the entrance field
    /// actually cuts the surface do we walk down until the first non-carved voxel,
    /// matching the later block carve.
    pub fn surface_after_caves(&self, x: i32, z: i32, surf_y: i32) -> i32 {
        if !self.cave_carved(x, surf_y, z, surf_y) {
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

        let y0 = CAVE_MIN_Y.max(0);
        let y1 = surf
            .iter()
            .copied()
            .max()
            .unwrap_or(0)
            .min(CHUNK_SY as i32 - 1);
        if y0 > y1 {
            return;
        }
        let lat = self.build_lattice(
            ox,
            y0,
            oz,
            ox + CHUNK_SX as i32 - 1,
            y1,
            oz + CHUNK_SZ as i32 - 1,
        );
        let blocks = chunk.blocks_slice_mut();

        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                let surf_y = surf[z * CHUNK_SX + x];
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
                    if self.carved_lat(&lat, wx, y, wz, surf_y) {
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

        let y0 = oy.max(CAVE_MIN_Y);
        let y1 = (oy + SECTION_SIZE as i32 - 1).min(surf.iter().copied().max().unwrap_or(i32::MIN));
        if y0 > y1 {
            return;
        }
        let lat = self.build_lattice(
            ox,
            y0,
            oz,
            ox + SECTION_SIZE as i32 - 1,
            y1,
            oz + SECTION_SIZE as i32 - 1,
        );
        let blocks = section.blocks_slice_mut();

        for z in 0..SECTION_SIZE {
            for x in 0..SECTION_SIZE {
                let surf_y = surf[z * SECTION_SIZE + x];
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
                    if self.carved_lat(&lat, wx, wy, wz, surf_y) {
                        blocks[i] = air;
                    }
                }
            }
        }
    }

    /// The entrance GATE: exactly-sampled (hot per-column in `ColumnGen`, where a
    /// lattice would cost more than the two samples it replaces). Returns the depth
    /// ease for the radius test when the gate opens, `None` otherwise.
    #[inline]
    fn entrance_gate_ease(&self, x: i32, y: i32, z: i32, surf_y: i32) -> Option<f64> {
        if surf_y < CAVE_ENTRANCE_MIN_SURFACE_Y {
            return None;
        }
        let depth = surf_y - y;
        if !(0..=CAVE_ENTRANCE_MAX_DEPTH).contains(&depth) {
            return None;
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
        (gate <= threshold).then_some(ease)
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The invariant everything hangs on: the point path (surface walks feeding
    /// column heightmaps) and the batch path (the actual block carve) must agree at
    /// every voxel, or heightmaps drift from carved blocks and skylight breaks.
    #[test]
    fn point_and_batch_carve_decisions_agree() {
        let field = CaveField::new(0x51EED);
        let (x0, y0, z0) = (-8, 0, 24); // deliberately straddles lattice cells
        let (x1, y1, z1) = (x0 + 15, y0 + 15, z0 + 15);
        let lat = field.build_lattice(x0, y0, z0, x1, y1, z1);
        for surf_y in [y1 - 2, y1 + 40] {
            let mut carved = 0;
            for y in y0..=y1 {
                for z in z0..=z1 {
                    for x in x0..=x1 {
                        let batch = field.carved_lat(&lat, x, y, z, surf_y);
                        let point = field.cave_carved(x, y, z, surf_y);
                        assert_eq!(batch, point, "divergence at ({x},{y},{z}) surf {surf_y}");
                        carved += batch as usize;
                    }
                }
            }
            // Not a shape pin — just proof the box exercised both outcomes.
            assert!(carved > 0, "test volume should contain some cave air");
        }
    }

    /// Batch lattices are world-anchored, so two different boxes covering the same
    /// voxel interpolate identical values: section seams cannot show.
    #[test]
    fn overlapping_lattices_agree_at_shared_voxels() {
        let field = CaveField::new(0xC0FFEE);
        let a = field.build_lattice(0, 0, 0, 15, 15, 15);
        let b = field.build_lattice(-16, 4, 8, 15, 35, 23);
        for &(x, y, z) in &[(0, 4, 8), (7, 15, 15), (15, 12, 9), (3, 8, 15)] {
            assert_eq!(a.a(x, y, z).to_bits(), b.a(x, y, z).to_bits());
            assert_eq!(a.rough(x, y, z).to_bits(), b.rough(x, y, z).to_bits());
            assert_eq!(a.cheese(x, y, z).to_bits(), b.cheese(x, y, z).to_bits());
        }
    }
}
