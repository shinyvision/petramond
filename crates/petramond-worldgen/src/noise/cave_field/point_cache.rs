//! Scalar surface walks revisit neighbouring voxels far more often than sources.

use std::sync::Arc;

use super::*;
use crate::cache::local::{self, LocalTable};

const TILE: i32 = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    context: crate::cache::GenContext,
    tile: [i32; 3],
    interior: bool,
}

thread_local! {
    static TILES: LocalTable<Key, Arc<CaveLattice>> = LocalTable::new(&local::CAVE_POINTS);
}

impl CaveField {
    pub(super) fn point_lattice(&self, p: [i32; 3], interior: bool) -> Arc<CaveLattice> {
        let key = Key {
            context: self.context(),
            tile: p.map(|v| v.div_euclid(TILE)),
            interior,
        };
        let hash = (key.tile[0] as u32 as u64)
            ^ (key.tile[1] as u32 as u64).rotate_left(21)
            ^ (key.tile[2] as u32 as u64).rotate_left(42)
            ^ self.seed as u64;
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
