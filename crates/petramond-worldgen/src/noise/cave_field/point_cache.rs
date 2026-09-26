//! Scalar surface walks revisit neighbouring voxels far more often than sources.

use std::sync::Arc;

use super::*;
use crate::cache::local::{self, LocalTable};

const TILE: i32 = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    seed: u32,
    habitats: usize,
    excavations: usize,
    tile: [i32; 3],
    interior: bool,
}

thread_local! {
    static TILES: LocalTable<Key, Arc<CaveLattice>> = LocalTable::new(&local::CAVE_POINTS);
}

impl CaveField {
    pub(super) fn point_lattice(&self, p: [i32; 3], interior: bool) -> Arc<CaveLattice> {
        let key = Key {
            seed: self.seed,
            habitats: std::ptr::from_ref(self.underground) as usize,
            excavations: std::ptr::from_ref(self.excavations) as usize,
            tile: p.map(|v| v.div_euclid(TILE)),
            interior,
        };
        let hash = (key.tile[0] as u32 as u64)
            ^ (key.tile[1] as u32 as u64).rotate_left(21)
            ^ (key.tile[2] as u32 as u64).rotate_left(42)
            ^ key.seed as u64;
        // Construction can query the natural source for room admission; the
        // table holds no borrow while the lattice is built.
        TILES.with(|tiles| {
            tiles.get_or_insert_with(local::spread(hash), key, || {
                let lo = key.tile.map(|v| v * TILE);
                let hi = lo.map(|v| v + TILE - 1);
                Arc::new(self.build_lattice_filtered(
                    lo[0],
                    lo[1],
                    lo[2],
                    hi[0],
                    hi[1],
                    hi[2],
                    Fields {
                        carve: true,
                        interior,
                        biome: false,
                        excavations: true,
                        positioned: true,
                        fluids: false,
                    },
                ))
            })
        })
    }
}
