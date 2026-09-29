use std::sync::Arc;

use super::*;

const CHAMBER_CELL: i32 = 64;
const CHAMBER_PAD: i32 = 32;
pub(super) type ChamberKey = (crate::cache::GenContext, [i32; 2]);

impl CaveField {
    pub(super) fn chamber_field_for(
        &self,
        bounds: [[i32; 3]; 2],
        y_span: (i32, i32),
    ) -> chamber::ChamberField {
        let [lo, hi] = bounds;
        let gather = |lo: [i32; 3], hi: [i32; 3]| {
            chamber::ChamberField::gather(
                &self.caches.caves.candidates,
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
        let key = (self.context(), cell);
        self.memos()
            .chamber_fields
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

    #[cfg(test)]
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
                    .is_some_and(|(lo, hi)| y0 - 1 <= hi && y1 + 1 >= lo),
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
            lanes: std::array::from_fn(|k| {
                Vec::with_capacity(if k >= lane::CLIMATE {
                    cap(fields.biome)
                } else {
                    0
                })
            }),
            no_aquifer,
            unlined,
            plain,
            geology,
            regions: regions::Columns::gather(self, [x0, z0], [x1, z1]),
            ordinary: if fields.biome {
                self.ordinary_cells([lx0, ly0, lz0], [nx - 1, ny - 1, nz - 1])
            } else {
                Vec::new()
            },
            #[cfg(test)]
            chamber_live: false,
            fields,
            walks: WalkCells::default(),
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
            pools: if fields.fluids {
                fluid_pools::Pools::around(self, [x0, y0, z0], [x1, y1, z1])
            } else {
                fluid_pools::Pools::default()
            },
        };

        if fields.carve {
            lat.walks = WalkCells::gather(
                &self.caches.caves.walks,
                self.context(),
                [lx0, ly0, lz0].map(|v| v * LATTICE_STEP),
                [nx - 1, ny - 1, nz - 1],
                LATTICE_STEP,
            );
        }
        if fields.carve {
            self.fill_source(&mut lat, fields);
        }
        if fields.biome {
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
            for ly in 0..ny {
                let wy = f64::from((ly0 + ly as i32) * LATTICE_STEP);
                for climate in &climates {
                    let mut climate = *climate;
                    climate[5] = (climate[5] - wy) / 128.0;
                    for (lane, value) in lat.lanes[lane::CLIMATE..].iter_mut().zip(climate) {
                        lane.push(value);
                    }
                }
            }
        }
        lat
    }
}
