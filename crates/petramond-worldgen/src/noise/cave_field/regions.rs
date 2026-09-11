//! Quart-resolution habitat columns, independent of excavation and voxel tiles.

use std::{
    cell::RefCell,
    sync::{Arc, LazyLock},
};

use super::CaveField;
use crate::{data::underground::IdSet, memo::SharedMemo};

#[derive(Clone, Copy, Default)]
pub(super) struct Column {
    candidates: IdSet,
    height: f64,
}

impl Column {
    #[inline]
    pub(super) fn has_candidates(&self) -> bool {
        !self.candidates.is_empty()
    }

    pub(super) fn id(self, field: &CaveField, y: i32) -> Option<u8> {
        field.underground.region_id(self.candidates, self.height, y)
    }

    /// [`Self::id`] at `y`, and the last height up to `y_max` over which that
    /// answer provably holds: the regional rule reads Y only through its
    /// aligned quart block and each candidate row's height band.
    pub(super) fn id_span(self, field: &CaveField, y: i32, y_max: i32) -> (Option<u8>, i32) {
        let id = self.id(field, y);
        let mut end = (y.div_euclid(4) + 1) * 4 - 1;
        for candidate in self.candidates.iter() {
            let (lo, hi) = field.underground.row_y(candidate);
            if lo > y {
                end = end.min(lo - 1);
            }
            if hi >= y {
                end = end.min(hi);
            }
        }
        (id, end.min(y_max))
    }

    fn include(self, field: &CaveField, y: [i32; 2], ids: &mut IdSet) {
        field
            .underground
            .region_ids_in(self.candidates, self.height, y, ids);
    }
}

type Key = (u32, usize, [i32; 2]);
type Tile = [Column; 16];
static TILES: LazyLock<SharedMemo<Key, Arc<Tile>>> = LazyLock::new(|| SharedMemo::new(16_384));
type Entry = Option<(Key, Arc<Tile>)>;
thread_local! {
    static LOCAL: RefCell<Vec<Entry>> = RefCell::new(vec![None; 256]);
}

#[derive(Default)]
pub(super) struct Columns {
    origin: [i32; 2],
    nx: usize,
    columns: Vec<Column>,
}

impl Columns {
    pub(super) fn gather(field: &CaveField, lo: [i32; 2], hi: [i32; 2]) -> Self {
        if field.underground.regions.is_empty() {
            return Self::default();
        }
        let origin = lo.map(|v| v.div_euclid(4));
        let end = hi.map(|v| v.div_euclid(4));
        let nx = (end[0] - origin[0] + 1) as usize;
        let columns = (origin[1]..=end[1])
            .flat_map(|z| (origin[0]..=end[0]).map(move |x| field.region_column(x * 4, z * 4)))
            .collect();
        Self {
            origin,
            nx,
            columns,
        }
    }

    #[inline]
    pub(super) fn at(&self, x: i32, z: i32) -> Column {
        if self.columns.is_empty() {
            return Column::default();
        }
        let dx = x.div_euclid(4) - self.origin[0];
        let dz = z.div_euclid(4) - self.origin[1];
        self.columns[dz as usize * self.nx + dx as usize]
    }
}

impl CaveField {
    pub(super) fn region_column(&self, x: i32, z: i32) -> Column {
        if self.underground.regions.is_empty() {
            return Column::default();
        }
        let pos = [x, z].map(|v| v.div_euclid(16));
        let key = (self.seed, self.table_identities()[0], pos);
        let hash =
            (pos[0] as u32 as u64) ^ (pos[1] as u32 as u64).rotate_left(32) ^ self.seed as u64;
        let slot = (hash.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 56) as usize;
        let saved = LOCAL.with(|cache| {
            cache.borrow()[slot]
                .as_ref()
                .filter(|(k, _)| *k == key)
                .map(|(_, tile)| Arc::clone(tile))
        });
        let tile = saved.unwrap_or_else(|| {
            let tile = TILES.get_or_compute_unlocked(key, || {
                Arc::new(std::array::from_fn(|i| {
                    let quart = [pos[0] * 4 + (i % 4) as i32, pos[1] * 4 + (i / 4) as i32];
                    let mut candidates = IdSet::default();
                    for group in &self.underground.regions {
                        group.candidates(
                            self.seed,
                            quart,
                            |[x, z]| self.climate_column(x, z),
                            &mut candidates,
                        );
                    }
                    let height = if candidates.iter().next().is_some() {
                        self.climate_column(quart[0] * 4, quart[1] * 4)[5]
                    } else {
                        0.0
                    };
                    Column { candidates, height }
                }))
            });
            LOCAL.with(|cache| cache.borrow_mut()[slot] = Some((key, Arc::clone(&tile))));
            tile
        });
        tile[(z.rem_euclid(16) / 4 * 4 + x.rem_euclid(16) / 4) as usize]
    }

    /// Habitat identity before excavation can claim any additional volume.
    pub(super) fn base_biome_at(&self, [x, y, z]: [i32; 3]) -> u8 {
        self.region_column(x, z)
            .id(self, y)
            .unwrap_or_else(|| self.underground.id_at(self.climate_at_corner(x, y, z), y))
    }

    pub(super) fn include_region_biomes(&self, lo: [i32; 3], hi: [i32; 3], ids: &mut IdSet) {
        if self.underground.regions.is_empty() {
            return;
        }
        for z in lo[2].div_euclid(4)..=hi[2].div_euclid(4) {
            for x in lo[0].div_euclid(4)..=hi[0].div_euclid(4) {
                self.region_column(x * 4, z * 4)
                    .include(self, [lo[1], hi[1]], ids);
            }
        }
    }
}
