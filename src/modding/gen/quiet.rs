//! Sections a feature has declared it writes nothing in: they are never dispatched to it again.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::RwLock;

use mod_api::SectionBox;
use rustc_hash::FxHashMap;

/// Declared boxes are kept per tile of `TILE` × `TILE` section columns, clipped to the tile.
const TILE: i32 = 16;
/// How many tiles past the declaring section's own a box is kept for. Dropping the rest only
/// means those sections are asked, which bounds what one reply can make the host store.
const REACH_TILES: i32 = 2;

#[derive(Default)]
pub(super) struct Quiet {
    any: AtomicBool,
    tiles: RwLock<FxHashMap<[i32; 2], Vec<SectionBox>>>,
}

fn tile_of(section: [i32; 3]) -> [i32; 2] {
    [section[0].div_euclid(TILE), section[2].div_euclid(TILE)]
}

fn covers(outer: &SectionBox, inner: &SectionBox) -> bool {
    (0..3).all(|a| outer.min[a] <= inner.min[a] && inner.max[a] <= outer.max[a])
}

impl Quiet {
    pub(super) fn contains(&self, section: [i32; 3]) -> bool {
        if !self.any.load(Ordering::Relaxed) {
            return false;
        }
        let tiles = self.tiles.read().unwrap_or_else(|e| e.into_inner());
        tiles
            .get(&tile_of(section))
            .is_some_and(|boxes| boxes.iter().any(|b| b.contains(section)))
    }

    pub(super) fn declare(&self, from: [i32; 3], boxes: &[SectionBox]) {
        if boxes.is_empty() {
            return;
        }
        let home = tile_of(from);
        let mut tiles = self.tiles.write().unwrap_or_else(|e| e.into_inner());
        for b in boxes {
            let t_lo = [
                b.min[0].div_euclid(TILE).max(home[0] - REACH_TILES),
                b.min[2].div_euclid(TILE).max(home[1] - REACH_TILES),
            ];
            let t_hi = [
                b.max[0].div_euclid(TILE).min(home[0] + REACH_TILES),
                b.max[2].div_euclid(TILE).min(home[1] + REACH_TILES),
            ];
            for tz in t_lo[1]..=t_hi[1] {
                for tx in t_lo[0]..=t_hi[0] {
                    let clipped = SectionBox {
                        min: [b.min[0].max(tx * TILE), b.min[1], b.min[2].max(tz * TILE)],
                        max: [
                            b.max[0].min(tx * TILE + TILE - 1),
                            b.max[1],
                            b.max[2].min(tz * TILE + TILE - 1),
                        ],
                    };
                    let list = tiles.entry([tx, tz]).or_default();
                    if !list.iter().any(|o| covers(o, &clipped)) {
                        list.retain(|o| !covers(&clipped, o));
                        list.push(clipped);
                    }
                }
            }
        }
        self.any.store(true, Ordering::Relaxed);
    }
}
