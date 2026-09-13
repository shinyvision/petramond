//! Fluid falls: a still source replacing cave rock and the pour it feeds, one
//! `fluid_falls` row at a time.
//!
//! Falls are derived once per chunk and memoized, and ONE claim walk
//! ([`ChunkFalls::cells`]) is read by the section stamp, the memoized terrain
//! mask and the sparse terrain query — so what a query promises never depends
//! on which of those happened to be cached.

use std::sync::{Arc, LazyLock};

use super::*;
use crate::data::underground::FluidFall as FallRow;
use crate::memo::SharedMemo;
use crate::rng::FeatureRng;
use mod_api::TerrainSpace;
use petramond_world::fluid_math::FALLING;

type Key = (u32, [usize; 2], [i32; 2]);
static FALLS: LazyLock<SharedMemo<Key, Arc<ChunkFalls>>> = LazyLock::new(|| SharedMemo::new(4096));

struct Fall {
    fluid: u16,
    source: [i32; 3],
    /// The exit, then the falling column under it, top down.
    pour: Box<[([i32; 3], u8)]>,
    lo: [i32; 3],
    hi: [i32; 3],
}

/// One cell a fall claims.
#[derive(Clone, Copy, Debug)]
pub struct FallCell {
    pub pos: [i32; 3],
    pub fluid: u16,
    pub meta: u8,
    source: bool,
}

impl FallCell {
    /// Whether the cell takes the fluid over terrain of `space`: the source
    /// replaces rock, the pour fills air.
    #[inline]
    pub fn admits(&self, space: TerrainSpace) -> bool {
        space
            == if self.source {
                TerrainSpace::Solid
            } else {
                TerrainSpace::Air
            }
    }
}

/// The falls whose sources lie in one chunk. Every cell of a fall lies in its
/// chunk's footprint.
#[derive(Default)]
pub struct ChunkFalls {
    falls: Box<[Fall]>,
}

impl ChunkFalls {
    /// Every cell of the inclusive box a fall claims, in write order.
    pub fn cells(&self, lo: [i32; 3], hi: [i32; 3], mut visit: impl FnMut(FallCell)) {
        let inside = |p: [i32; 3]| (0..3).all(|a| lo[a] <= p[a] && p[a] <= hi[a]);
        for fall in self.falls.iter() {
            if (0..3).any(|a| fall.hi[a] < lo[a] || fall.lo[a] > hi[a]) {
                continue;
            }
            let claims = std::iter::once((fall.source, 0, true))
                .chain(fall.pour.iter().map(|&(pos, meta)| (pos, meta, false)));
            for (pos, meta, source) in claims {
                if inside(pos) {
                    visit(FallCell {
                        pos,
                        fluid: fall.fluid,
                        meta,
                        source,
                    });
                }
            }
        }
    }

    /// What `pos` holds once the falls are stamped over terrain of `space`.
    pub fn space_at(&self, pos: [i32; 3], mut space: TerrainSpace) -> TerrainSpace {
        self.cells(pos, pos, |cell| {
            if cell.admits(space) {
                space = TerrainSpace::Fluid;
            }
        });
        space
    }
}

const SIDES: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

impl CaveField {
    /// The falls whose sources lie in chunk `(cx, cz)`.
    pub(crate) fn chunk_falls(&self, cx: i32, cz: i32) -> Arc<ChunkFalls> {
        let key = (self.seed, self.table_identities(), [cx, cz]);
        FALLS.get_or_insert(key, || Arc::new(self.derive_falls(cx, cz)))
    }

    /// The highest cell any fall row claims; `None` without fall rows.
    pub(crate) fn falls_top(&self) -> Option<i32> {
        self.underground.falls.iter().map(|row| row.y.1).max()
    }

    fn derive_falls(&self, cx: i32, cz: i32) -> ChunkFalls {
        let sec = SECTION_SIZE as i32;
        let mut falls = Vec::new();
        for z in 0..sec {
            for x in 0..sec {
                let (wx, wz) = (cx * sec + x, cz * sec + z);
                let mut rows = self.underground.falls.iter();
                if let Some(fall) = rows.find_map(|row| self.fall_at(row, wx, wz)) {
                    falls.push(fall);
                }
            }
        }
        ChunkFalls {
            falls: falls.into(),
        }
    }

    /// One column's fall of `row`, or `None` when the column rolls none or
    /// the cave cannot hold one. The source replaces natural rock of a cave
    /// ceiling or wall enclosed on every other side, so its exit is the one
    /// face it pours through:
    ///  - a ceiling source over open air falls straight out;
    ///  - a wall source with exactly one open side, that side air, pours into
    ///    that cell and falls from there.
    ///
    /// The row's load validation keeps the band clear of the carve's surface
    /// rules, so the verdict reads the cave at the row's minimum surface and
    /// the column's own surface only has to reach it.
    fn fall_at(&self, row: &FallRow, wx: i32, wz: i32) -> Option<Fall> {
        if !FeatureRng::positional(self.seed, row.salt, wx, 0, wz).chance(row.chance) {
            return None;
        }
        let y_s = row.y.0
            + FeatureRng::positional(self.seed, row.salt + 1, wx, 0, wz)
                .next_i32(0, row.y.1 - row.y.0);
        let surf_y = row.min_surface;
        let (lo, hi) = ([wx - 1, CAVE_MIN_Y, wz - 1], [wx + 1, y_s + 1, wz + 1]);
        let mut lat = self.build_lattice_filtered(
            lo[0],
            lo[1],
            lo[2],
            hi[0],
            hi[1],
            hi[2],
            Fields {
                fluids: false,
                ..Fields::ALL
            },
        );
        {
            // Natural rock only — never a positioned fill or an aquifer barrier.
            let mut here = Col::new(&lat, wx, wz);
            if !matches!(
                self.cut_col(&mut here, y_s, surf_y),
                CaveCut::Solid | CaveCut::Shell
            ) || self.cut_col(&mut here, y_s + 1, surf_y).is_open()
            {
                return None;
            }
        }
        // Only a column that got this far needs to tell a pool from air.
        lat.pools = fluid_pools::Pools::around(self, lo, hi);
        let lat = lat;
        let cut = |x: i32, z: i32, y: i32| self.cut_col(&mut Col::new(&lat, x, z), y, surf_y);
        let mut open = [((0, 0), CaveCut::Solid); SIDES.len()];
        let mut open_count = 0;
        for (dx, dz) in SIDES {
            let side = cut(wx + dx, wz + dz, y_s);
            if side.is_open() {
                open[open_count] = ((dx, dz), side);
                open_count += 1;
            }
        }
        let below = cut(wx, wz, y_s - 1);
        // A fluid under or beside the source is not a face it can pour
        // through, and it still counts against the enclosure.
        let (exit, exit_meta) = match (below.is_open(), &open[..open_count]) {
            (true, []) if below.is_air() => ([wx, y_s - 1, wz], FALLING),
            (false, [((dx, dz), side)]) if side.is_air() => {
                let (ex, ez) = (wx + dx, wz + dz);
                // A pour leaving its chunk could not be claimed by that chunk.
                let chunk = |v: i32| v.div_euclid(SECTION_SIZE as i32);
                if chunk(ex) != chunk(wx) || chunk(ez) != chunk(wz) {
                    return None;
                }
                // One flow step out of the source; the sim re-levels it.
                ([ex, y_s, ez], 1)
            }
            _ => return None,
        };
        // Air only: the pour stops at a pool's or an aquifer's surface as it
        // does at rock.
        let mut shaft = Col::new(&lat, exit[0], exit[2]);
        let mut pour = vec![(exit, exit_meta)];
        let mut y = exit[1] - 1;
        while y >= CAVE_MIN_Y && self.cut_col(&mut shaft, y, surf_y).is_air() {
            pour.push(([exit[0], y, exit[2]], FALLING));
            y -= 1;
        }
        // A wall pour falls; it never puddles on the floor beside its source.
        if exit_meta != FALLING && pour.len() < 2 {
            return None;
        }
        if self.density_surface(wx, wz) < row.min_surface {
            return None;
        }
        let source = [wx, y_s, wz];
        let cells = || std::iter::once(source).chain(pour.iter().map(|(p, _)| *p));
        let lo = std::array::from_fn(|a| cells().map(|p| p[a]).min().expect("a source"));
        let hi = std::array::from_fn(|a| cells().map(|p| p[a]).max().expect("a source"));
        Some(Fall {
            fluid: row.fluid,
            source,
            pour: pour.into(),
            lo,
            hi,
        })
    }
}

#[cfg(test)]
mod tests;
