//! Shared world lattice interpolation and conservative carving bounds.

use super::*;

impl<'a> Col<'a> {
    #[inline]
    pub(super) fn new(lat: &'a CaveLattice, x: i32, z: i32) -> Self {
        let cx = (x.div_euclid(LATTICE_STEP) - lat.lx0) as usize;
        let cz = (z.div_euclid(LATTICE_STEP) - lat.lz0) as usize;
        debug_assert!(cx + 1 < lat.nx && cz + 1 < lat.nz);
        Self {
            lat,
            x,
            z,
            region: lat.regions.at(x, z),
            i00: cz * lat.nx + cx,
            i01: (cz + 1) * lat.nx + cx,
            plane: lat.nz * lat.nx,
            tx: x.rem_euclid(LATTICE_STEP) as f64 / LATTICE_STEP_F,
            tz: z.rem_euclid(LATTICE_STEP) as f64 / LATTICE_STEP_F,
            cell_y: i32::MIN,
            cached: 0,
            lo: [0.0; lane::COUNT],
            hi: [0.0; lane::COUNT],
        }
    }

    #[inline]
    pub(super) fn bilinear(&self, field: &[f64], base: usize) -> f64 {
        let lerp = |a: f64, b: f64, t: f64| a + (b - a) * t;
        let x0 = lerp(field[base + self.i00], field[base + self.i00 + 1], self.tx);
        let x1 = lerp(field[base + self.i01], field[base + self.i01 + 1], self.tx);
        lerp(x0, x1, self.tz)
    }

    #[inline]
    pub(super) fn get(&mut self, k: usize, y: i32) -> f64 {
        let cy = y.div_euclid(LATTICE_STEP) - self.lat.ly0;
        if cy != self.cell_y {
            self.cell_y = cy;
            self.cached = 0;
        }
        if self.cached & (1 << k) == 0 {
            let lat = self.lat;
            let field: &[f64] = match k {
                lane::ENTRANCE => &lat.entrance,
                lane::INTERIOR => &lat.density,
                lane::NOODLE_TOGGLE => &lat.noodle_toggle,
                lane::NOODLE_A => &lat.noodle_a,
                lane::NOODLE_B => &lat.noodle_b,
                lane::NOODLE_WIDTH => &lat.noodle_width,
                lane::CLIMATE..lane::COUNT => &lat.climate[k - lane::CLIMATE],
                _ => unreachable!("unknown cave lane"),
            };
            debug_assert!(
                !field.is_empty(),
                "lattice built without a field group this decision reads"
            );
            debug_assert!((cy as usize) + 1 < lat.ny);
            let base = cy as usize * self.plane;
            self.lo[k] = self.bilinear(field, base);
            self.hi[k] = self.bilinear(field, base + self.plane);
            self.cached |= 1 << k;
        }
        let ty = y.rem_euclid(LATTICE_STEP) as f64 / LATTICE_STEP_F;
        self.lo[k] + (self.hi[k] - self.lo[k]) * ty
    }

    pub(super) fn climate(&mut self, y: i32) -> underground::ClimatePoint {
        std::array::from_fn(|axis| self.get(lane::CLIMATE + axis, y))
    }

    /// The lattice cell the cursor last read, in the per-cell arrays' order.
    #[inline]
    pub(super) fn cell(&self) -> usize {
        let (mx, mz) = (self.lat.nx - 1, self.lat.nz - 1);
        let (cx, cz) = (self.i00 % self.lat.nx, self.i00 / self.lat.nx);
        (self.cell_y as usize * mz + cz) * mx + cx
    }
}

impl CaveLattice {
    #[inline]
    pub(super) fn tri(&self, field: &[f64], x: i32, y: i32, z: i32) -> f64 {
        debug_assert!(
            !field.is_empty(),
            "lattice built without a field group this decision reads"
        );
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

    pub(super) fn may_cut_mask(&self, biomes: &UndergroundBiomes, floor_lining: bool) -> Vec<bool> {
        let (mx, my, mz) = (self.nx - 1, self.ny - 1, self.nz - 1);
        let bounds = |field: &[f64], cx: usize, cy: usize, cz: usize| {
            let mut range = (f64::INFINITY, f64::NEG_INFINITY);
            for d in 0..8 {
                let i =
                    ((cy + (d >> 2 & 1)) * self.nz + cz + (d >> 1 & 1)) * self.nx + cx + (d & 1);
                range.0 = range.0.min(field[i]);
                range.1 = range.1.max(field[i]);
            }
            range
        };
        let abs_min = |(lo, hi): (f64, f64)| lo.max(-hi).max(0.0);
        let shell = 0.05 * biomes.bounds;
        let mut mask = vec![false; mx * my * mz];
        for cy in 0..my {
            for cz in 0..mz {
                for cx in 0..mx {
                    let density = bounds(&self.entrance, cx, cy, cz)
                        .0
                        .min(bounds(&self.density, cx, cy, cz).0);
                    let noodles = bounds(&self.noodle_toggle, cx, cy, cz).1 >= 0.0
                        && 1.5
                            * abs_min(bounds(&self.noodle_a, cx, cy, cz)).max(abs_min(bounds(
                                &self.noodle_b,
                                cx,
                                cy,
                                cz,
                            )))
                            < bounds(&self.noodle_width, cx, cy, cz).1 + shell;
                    let walks = self.walks.is_some() && self.walk_cells[(cy * mz + cz) * mx + cx];
                    // A placed field is a cut of its own: its cells are visited
                    // like any cell the natural sources may open.
                    let field = self.volumes.touched([
                        self.lx0 + cx as i32,
                        self.ly0 + cy as i32,
                        self.lz0 + cz as i32,
                    ]);
                    mask[(cy * mz + cz) * mx + cx] = density < shell || noodles || walks || field;
                }
            }
        }
        if floor_lining {
            let base = mask.clone();
            let reach = (biomes.lining_floor_depth_max as usize).div_ceil(LATTICE_STEP as usize);
            for cy in 0..my {
                for cz in 0..mz {
                    for cx in 0..mx {
                        mask[(cy * mz + cz) * mx + cx] |= (1..=reach)
                            .any(|dy| cy + dy >= my || base[((cy + dy) * mz + cz) * mx + cx]);
                    }
                }
            }
        }
        mask
    }

    #[cfg(test)]
    pub(super) fn chamber_is_live(&self) -> bool {
        self.chamber_live
    }

    pub(super) fn climate_at(&self, x: i32, y: i32, z: i32) -> underground::ClimatePoint {
        std::array::from_fn(|axis| self.tri(&self.climate[axis], x, y, z))
    }
}
