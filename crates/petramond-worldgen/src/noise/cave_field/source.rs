//! The natural cave field sampled at lattice corners, memoized per
//! world-anchored block of `BLOCK³` corners.
//!
//! A corner's sample is a pure function of the context and its position (its
//! depth comes from the memoized climate column, its rooms from a chamber
//! field whose restriction to any box is value-neutral), so a block computes
//! the same values any lattice would, and one lookup serves a whole block.

use std::sync::Arc;

use super::{lane, CaveField, CaveLattice, Fields, LATTICE_STEP};
use crate::cache::GenContext;
use crate::noise::cave_density::Sample;

const BLOCK: i32 = 4;
const CORNERS: usize = (BLOCK * BLOCK * BLOCK) as usize;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct Key {
    context: GenContext,
    block: [i32; 3],
}

/// One block's lanes, indexed `(y * BLOCK + z) * BLOCK + x` in corner units.
pub(super) struct Block {
    entrance: [f64; CORNERS],
    interior: [f64; CORNERS],
    noodle: [[f64; CORNERS]; 4],
    #[cfg(test)]
    live: [bool; CORNERS],
}

impl CaveField {
    /// A block always carries the interior lanes: a lattice built without
    /// them reads only the entrance, and the same corners are carved with them
    /// once their sections generate.
    fn source_block(&self, block: [i32; 3], excavations: bool) -> Arc<Block> {
        let key = Key {
            context: if excavations {
                self.context()
            } else {
                self.context().without_excavations()
            },
            block,
        };
        self.memos()
            .source
            .get_or_insert(key, || Arc::new(self.compute_block(block, excavations)))
    }

    fn compute_block(&self, block: [i32; 3], excavations: bool) -> Block {
        let lo = block.map(|b| b * BLOCK * LATTICE_STEP);
        let hi = lo.map(|v| v + (BLOCK - 1) * LATTICE_STEP);
        let rooms = self
            .chamber_y_span
            .filter(|_| excavations)
            .and_then(|(ylo, yhi)| {
                if hi[1] < ylo || lo[1] > yhi {
                    return None;
                }
                let rooms = self.chamber_field_for([lo, hi], (ylo, yhi));
                (!rooms.is_empty()).then_some(rooms)
            });
        let mut heights = [0.0; (BLOCK * BLOCK) as usize];
        for bz in 0..BLOCK {
            for bx in 0..BLOCK {
                heights[(bz * BLOCK + bx) as usize] =
                    self.climate_column(lo[0] + bx * LATTICE_STEP, lo[2] + bz * LATTICE_STEP)[5];
            }
        }
        let mut points = [[0.0; 3]; CORNERS];
        let mut depth = [0.0; CORNERS];
        let mut excavation = [(0.0, 0.0); CORNERS];
        for (i, point) in points.iter_mut().enumerate() {
            let [bx, bz, by] = [
                i as i32 % BLOCK,
                i as i32 / BLOCK % BLOCK,
                i as i32 / (BLOCK * BLOCK),
            ];
            let w = [
                lo[0] + bx * LATTICE_STEP,
                lo[1] + by * LATTICE_STEP,
                lo[2] + bz * LATTICE_STEP,
            ];
            *point = w.map(f64::from);
            depth[i] = (heights[(bz * BLOCK + bx) as usize] - point[1]) / 128.0;
            if let Some(rooms) = &rooms {
                excavation[i] = rooms.at(w[0], w[1], w[2], self.natural.knead(*point));
            }
        }
        let mut samples = [Sample::default(); CORNERS];
        self.natural
            .sample_batch(&points, &depth, &excavation, &mut samples);
        Block {
            entrance: samples.map(|s| s.entrance),
            interior: samples.map(|s| s.interior),
            noodle: std::array::from_fn(|lane| samples.map(|s| s.noodle[lane])),
            #[cfg(test)]
            live: samples.map(|s| s.chamber_live),
        }
    }

    /// Fills the lattice's source lanes (entrance, interior, noodles) from the
    /// blocks its corners fall in.
    pub(super) fn fill_source(&self, lat: &mut CaveLattice, fields: Fields) {
        let n = lat.nx * lat.ny * lat.nz;
        for lane in &mut lat.lanes[..lane::CLIMATE] {
            lane.clear();
            lane.resize(n, 0.0);
        }
        let [entrance, density, toggle, noodle_a, noodle_b, width, ..] = &mut lat.lanes;
        let origin = [lat.lx0, lat.ly0, lat.lz0];
        let dims = [lat.nx, lat.ny, lat.nz].map(|d| d as i32);
        let first = origin.map(|v| v.div_euclid(BLOCK));
        let last: [i32; 3] = std::array::from_fn(|a| (origin[a] + dims[a] - 1).div_euclid(BLOCK));
        for by in first[1]..=last[1] {
            for bz in first[2]..=last[2] {
                for bx in first[0]..=last[0] {
                    let b = [bx, by, bz];
                    let block = self.source_block(b, fields.excavations);
                    let span = |a: usize| {
                        let lo = (b[a] * BLOCK).max(origin[a]);
                        let hi = (b[a] * BLOCK + BLOCK - 1).min(origin[a] + dims[a] - 1);
                        lo..=hi
                    };
                    for cy in span(1) {
                        for cz in span(2) {
                            let row = ((cy - origin[1]) * dims[2] + cz - origin[2]) * dims[0];
                            let brow = ((cy - by * BLOCK) * BLOCK + cz - bz * BLOCK) * BLOCK;
                            for cx in span(0) {
                                let i = (row + cx - origin[0]) as usize;
                                let j = (brow + cx - bx * BLOCK) as usize;
                                if !fields.interior {
                                    let mut sample = Sample::entrance_only(block.entrance[j]);
                                    sample.finish(f64::from(cy * LATTICE_STEP), (0.0, 0.0));
                                    entrance[i] = sample.entrance;
                                    density[i] = sample.interior;
                                    [noodle_a[i], noodle_b[i], toggle[i], width[i]] = sample.noodle;
                                    continue;
                                }
                                entrance[i] = block.entrance[j];
                                density[i] = block.interior[j];
                                noodle_a[i] = block.noodle[0][j];
                                noodle_b[i] = block.noodle[1][j];
                                toggle[i] = block.noodle[2][j];
                                width[i] = block.noodle[3][j];
                                #[cfg(test)]
                                {
                                    lat.chamber_live |= block.live[j];
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
