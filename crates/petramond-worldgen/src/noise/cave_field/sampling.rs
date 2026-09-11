//! Sample only the source groups required by a query.

use std::sync::{Arc, LazyLock};

use super::*;
use crate::memo::SharedMemo;

/// Excavation fields are gathered once per 64-block column cell padded by
/// this many blocks and shared by every generator; a lattice inside the
/// padded cell restricts that field to its own box.
const CHAMBER_CELL: i32 = 64;
const CHAMBER_PAD: i32 = 32;
type ChamberKey = (u32, usize, usize, [i32; 2]);
static CHAMBER_FIELDS: LazyLock<SharedMemo<ChamberKey, Arc<chamber::ChamberField>>> =
    LazyLock::new(|| SharedMemo::new(1024));

impl CaveField {
    /// The excavation terms reaching `bounds`: restricted from the shared
    /// per-cell field when the box fits one padded cell, gathered directly
    /// otherwise (a wide positional batch).
    fn chamber_field_for(
        &self,
        bounds: [[i32; 3]; 2],
        y_span: (i32, i32),
    ) -> chamber::ChamberField {
        let [lo, hi] = bounds;
        let gather = |lo: [i32; 3], hi: [i32; 3]| {
            chamber::ChamberField::gather(
                chamber::CandidateCache::shared(),
                self.underground,
                self.excavations,
                self.seed,
                [lo, hi],
                |x, y, z| self.base_biome_at([x, y, z]),
                |p| self.natural_open(p),
            )
        };
        let cell = [
            lo[0].div_euclid(CHAMBER_CELL),
            lo[2].div_euclid(CHAMBER_CELL),
        ];
        let fits =
            |axis: usize, c: i32| hi[axis] <= c * CHAMBER_CELL + CHAMBER_CELL - 1 + CHAMBER_PAD;
        if !(fits(0, cell[0]) && fits(2, cell[1])) {
            return gather(lo, hi);
        }
        let key = (
            self.seed,
            std::ptr::from_ref(self.underground) as usize,
            std::ptr::from_ref(self.excavations) as usize,
            cell,
        );
        CHAMBER_FIELDS
            .get_or_insert(key, || {
                Arc::new(gather(
                    [
                        cell[0] * CHAMBER_CELL - CHAMBER_PAD,
                        y_span.0,
                        cell[1] * CHAMBER_CELL - CHAMBER_PAD,
                    ],
                    [
                        cell[0] * CHAMBER_CELL + CHAMBER_CELL - 1 + CHAMBER_PAD,
                        y_span.1,
                        cell[1] * CHAMBER_CELL + CHAMBER_CELL - 1 + CHAMBER_PAD,
                    ],
                ))
            })
            .restrict(lo, hi)
    }

    pub(super) fn build_lattice(
        &self,
        x0: i32,
        y0: i32,
        z0: i32,
        x1: i32,
        y1: i32,
        z1: i32,
    ) -> CaveLattice {
        self.build_lattice_filtered(x0, y0, z0, x1, y1, z1, Fields::ALL)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn build_lattice_filtered(
        &self,
        x0: i32,
        y0: i32,
        z0: i32,
        x1: i32,
        y1: i32,
        z1: i32,
        mut fields: Fields,
    ) -> CaveLattice {
        // What a carve can meet in this box, from the habitat leaf grid: with
        // no aquifer row possible the barrier neighbourhood is never read, with
        // no lining row possible no shell cell is ever painted, and with only
        // stone possible the shell width is the base width without asking.
        let (no_aquifer, unlined, plain, geology) = if fields.carve {
            let ids = self
                .underground_biome_ids_in_box([x0 - 1, y0 - 1, z0 - 1], [x1 + 1, y1 + 1, z1 + 1]);
            let ids = ids.ids();
            (
                !ids.iter().any(|&id| self.underground.aquifer(id).is_some()),
                !ids.iter().any(|&id| {
                    self.underground.lining(id) != 0 || self.underground.faces(id).is_some()
                }),
                ids == [0],
                ids.iter()
                    .any(|&id| self.underground.geology_ids.contains(id)),
            )
        } else {
            (false, false, false, false)
        };
        let pad = i32::from(
            fields.carve
                && !no_aquifer
                && self
                    .underground
                    .aquifer_y_span
                    .is_some_and(|(lo, hi)| y0 <= hi && y1 >= lo),
        );
        if pad != 0 {
            fields.biome = true;
        }
        let (x0, y0, z0, x1, y1, z1) = (x0 - pad, y0 - pad, z0 - pad, x1 + pad, y1 + pad, z1 + pad);
        let lx0 = x0.div_euclid(LATTICE_STEP);
        let ly0 = y0.div_euclid(LATTICE_STEP);
        let lz0 = z0.div_euclid(LATTICE_STEP);
        let nx = (x1.div_euclid(LATTICE_STEP) + 1 - lx0) as usize + 1;
        let ny = (y1.div_euclid(LATTICE_STEP) + 1 - ly0) as usize + 1;
        let nz = (z1.div_euclid(LATTICE_STEP) + 1 - lz0) as usize + 1;
        let n = nx * ny * nz;
        let cap = |want: bool| if want { n } else { 0 };
        let mut lat = CaveLattice {
            lx0,
            ly0,
            lz0,
            nx,
            ny,
            nz,
            entrance: Vec::with_capacity(cap(fields.carve)),
            density: Vec::with_capacity(cap(fields.carve)),
            noodle_a: Vec::with_capacity(cap(fields.carve)),
            noodle_b: Vec::with_capacity(cap(fields.carve)),
            noodle_toggle: Vec::with_capacity(cap(fields.carve)),
            noodle_width: Vec::with_capacity(cap(fields.carve)),
            climate: std::array::from_fn(|_| Vec::with_capacity(cap(fields.biome))),
            no_aquifer,
            unlined,
            plain,
            geology,
            regions: regions::Columns::gather(self, [x0, z0], [x1, z1]),
            walk_cells: Vec::new(),
            #[cfg(test)]
            chamber_live: false,
            fields,
            walks: None,
            claims: if fields.biome && fields.excavations && fields.positioned {
                super::volumes::claims::Columns::gather(self, [x0, z0], [x1, z1])
            } else {
                super::volumes::claims::Columns::default()
            },
            volumes: if fields.carve && fields.excavations && fields.positioned {
                super::volumes::Tiles::gather(self, [x0, y0, z0], [x1, y1, z1])
            } else {
                super::volumes::Tiles::default()
            },
        };

        let rooms = self
            .chamber_y_span
            .filter(|_| fields.interior && fields.excavations)
            .and_then(|(lo, hi)| {
                let clo = [lx0 * LATTICE_STEP, ly0 * LATTICE_STEP, lz0 * LATTICE_STEP];
                let chi = [
                    clo[0] + (nx as i32 - 1) * LATTICE_STEP,
                    clo[1] + (ny as i32 - 1) * LATTICE_STEP,
                    clo[2] + (nz as i32 - 1) * LATTICE_STEP,
                ];
                if chi[1] < lo || clo[1] > hi {
                    return None;
                }
                let rooms = self.chamber_field_for([clo, chi], (lo, hi));
                (!rooms.is_empty()).then_some(rooms)
            });

        let climates: Vec<_> = (0..nz)
            .flat_map(|z| {
                (0..nx).map(move |x| {
                    self.climate_column(
                        (lx0 + x as i32) * LATTICE_STEP,
                        (lz0 + z as i32) * LATTICE_STEP,
                    )
                })
            })
            .collect();
        lat.walks = fields.carve.then(|| {
            WalkField::gather(
                self.seed,
                [
                    [lx0 * LATTICE_STEP, ly0 * LATTICE_STEP, lz0 * LATTICE_STEP],
                    [
                        (lx0 + nx as i32 - 1) * LATTICE_STEP,
                        (ly0 + ny as i32 - 1) * LATTICE_STEP,
                        (lz0 + nz as i32 - 1) * LATTICE_STEP,
                    ],
                ],
            )
        });
        for ly in 0..ny {
            let wy = (ly0 + ly as i32) * LATTICE_STEP;
            for lz in 0..nz {
                let wz = (lz0 + lz as i32) * LATTICE_STEP;
                for lx in 0..nx {
                    let wx = (lx0 + lx as i32) * LATTICE_STEP;
                    let p = [wx as f64, wy as f64, wz as f64];
                    if fields.carve {
                        let sample = self.source_sample(
                            [wx, wy, wz],
                            (climates[lz * nx + lx][5] - p[1]) / 128.0,
                            fields,
                            || {
                                rooms
                                    .as_ref()
                                    .map_or((0.0, 0.0), |r| r.at(wx, wy, wz, self.natural.knead(p)))
                            },
                        );
                        lat.entrance.push(sample.entrance);
                        lat.density.push(sample.interior);
                        lat.noodle_a.push(sample.noodle[0]);
                        lat.noodle_b.push(sample.noodle[1]);
                        lat.noodle_toggle.push(sample.noodle[2]);
                        lat.noodle_width.push(sample.noodle[3]);
                        #[cfg(test)]
                        {
                            lat.chamber_live |= sample.chamber_live;
                        }
                    }
                    if fields.biome {
                        let mut climate = climates[lz * nx + lx];
                        climate[5] = (climate[5] - p[1]) / 128.0;
                        for (lane, value) in lat.climate.iter_mut().zip(climate) {
                            lane.push(value);
                        }
                    }
                }
            }
        }
        if let Some(walks) = &lat.walks {
            // Which lattice cells any cut can reach, so a voxel outside them
            // never asks the walk index.
            let (mx, my, mz) = (nx - 1, ny - 1, nz - 1);
            lat.walk_cells = (0..mx * my * mz)
                .map(|i| {
                    let (cx, cz, cy) = (i % mx, (i / mx) % mz, i / (mx * mz));
                    let lo = [
                        (lx0 + cx as i32) * LATTICE_STEP,
                        (ly0 + cy as i32) * LATTICE_STEP,
                        (lz0 + cz as i32) * LATTICE_STEP,
                    ];
                    walks.intersects([lo, lo.map(|v| v + LATTICE_STEP - 1)])
                })
                .collect();
        }
        lat
    }
}
