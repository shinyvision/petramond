//! Scalar surface walks revisit neighbouring voxels far more often than sources.

use std::cell::RefCell;
use std::sync::Arc;

use super::*;

const TILE: i32 = 8;
const CAPACITY: usize = 128;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    seed: u32,
    habitats: usize,
    excavations: usize,
    tile: [i32; 3],
    interior: bool,
}

type Entry = Option<(Key, Arc<CaveLattice>)>;
thread_local! {
    static TILES: std::cell::RefCell<Box<[Entry]>> = RefCell::new(vec![None; CAPACITY].into_boxed_slice());
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
        let slot = (hash.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 57) as usize;
        TILES.with(|tiles| {
            if let Some((saved, value)) = &tiles.borrow()[slot] {
                if *saved == key {
                    return Arc::clone(value);
                }
            }
            // Construction can query the natural source for room admission, so
            // no cache borrow may remain held while the lattice is built.
            let lo = key.tile.map(|v| v * TILE);
            let hi = lo.map(|v| v + TILE - 1);
            let value = Arc::new(self.build_lattice_filtered(
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
            ));
            tiles.borrow_mut()[slot] = Some((key, Arc::clone(&value)));
            value
        })
    }
}
