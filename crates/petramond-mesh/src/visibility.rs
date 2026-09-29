use std::sync::LazyLock;

use petramond_world::block::{Block, MeshEmitter};
use petramond_world::chunk::{SECTION_SIZE, SECTION_VOLUME};
use petramond_world::section::Section;

use crate::face::Face;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SectionVisibility(u64);

const ALL_BITS: u64 = (1 << 36) - 1;

const ROWS: usize = SECTION_SIZE * SECTION_SIZE;
const _: () = assert!(SECTION_SIZE == 16, "rows are u16 bitsets along X");

impl SectionVisibility {
    pub const ALL: Self = Self(ALL_BITS);
    pub const NONE: Self = Self(0);

    #[inline]
    fn bit(a: Face, b: Face) -> u64 {
        1 << (a as u32 * 6 + b as u32)
    }

    #[inline]
    pub fn connects(self, a: Face, b: Face) -> bool {
        self.0 & Self::bit(a, b) != 0
    }

    fn connect(&mut self, a: Face, b: Face) {
        self.0 |= Self::bit(a, b) | Self::bit(b, a);
    }

    pub fn from_pairs(pairs: &[(Face, Face)]) -> Self {
        let mut visibility = Self::NONE;
        for &(a, b) in pairs {
            visibility.connect(a, b);
        }
        visibility
    }

    pub fn of_section(section: &Section) -> Self {
        if section.is_empty_air() || !section.has_opaque_blocks() {
            return Self::ALL;
        }
        if section.all_opaque() {
            return Self::NONE;
        }
        let occluders = occluders();
        let mut open = [0u16; ROWS];
        let mut any_closed = false;
        for (row, word) in open.iter_mut().enumerate() {
            let base = row * SECTION_SIZE;
            for x in 0..SECTION_SIZE {
                let id = section.block_at_idx(base + x);
                if occluders.get(id as usize).copied().unwrap_or(false) {
                    any_closed = true;
                } else {
                    *word |= 1 << x;
                }
            }
        }
        if !any_closed {
            return Self::ALL;
        }
        flood(open)
    }
}

impl Default for SectionVisibility {
    fn default() -> Self {
        Self::ALL
    }
}

fn occluders() -> &'static [bool] {
    static OCCLUDERS: LazyLock<Box<[bool]>> = LazyLock::new(|| {
        Block::all()
            .iter()
            .map(|&block| {
                block.is_opaque()
                    && !block.is_fluid()
                    && !block.flags().invisible()
                    && block.mesh_emitter() == MeshEmitter::Cube
            })
            .collect()
    });
    &OCCLUDERS
}

const _: () = assert!(ROWS * SECTION_SIZE == SECTION_VOLUME);

/// Which boundary faces a run of open cells in row `(y, z)` with X bits `bits` touches.
#[inline]
fn touched(y: usize, z: usize, bits: u16) -> u8 {
    let last = SECTION_SIZE - 1;
    let mut faces = 0u8;
    if bits & (1 << last) != 0 {
        faces |= 1 << Face::PosX as u8;
    }
    if bits & 1 != 0 {
        faces |= 1 << Face::NegX as u8;
    }
    if y == last {
        faces |= 1 << Face::PosY as u8;
    }
    if y == 0 {
        faces |= 1 << Face::NegY as u8;
    }
    if z == last {
        faces |= 1 << Face::PosZ as u8;
    }
    if z == 0 {
        faces |= 1 << Face::NegZ as u8;
    }
    faces
}

/// Open cells reachable within one row from `bits` through `open`: a run grows one cell each
/// way per step, so it settles in as many steps as the longest run.
#[inline]
fn spread(mut bits: u16, open: u16) -> u16 {
    loop {
        let grown = (bits | (bits << 1) | (bits >> 1)) & open;
        if grown == bits {
            return bits;
        }
        bits = grown;
    }
}

/// Flood every open component that touches the boundary, a 16-cell row at a time: rows are
/// `u16` bitsets along X, a component spreads inside a row with shifts and crosses to the four
/// neighbouring rows by masking, so a cave section costs a few hundred word operations instead
/// of a per-cell stack walk.
fn flood(mut open: [u16; ROWS]) -> SectionVisibility {
    let last = SECTION_SIZE - 1;
    let mut visibility = SectionVisibility::NONE;
    let mut stack: Vec<(usize, u16)> = Vec::with_capacity(64);
    for seed_row in 0..ROWS {
        let (y, z) = (seed_row / SECTION_SIZE, seed_row % SECTION_SIZE);
        let boundary_row = y == 0 || y == last || z == 0 || z == last;
        loop {
            let row = open[seed_row];
            let seeds = if boundary_row {
                row
            } else {
                row & ((1 << last) | 1)
            };
            if seeds == 0 {
                break;
            }
            let seed = seeds & seeds.wrapping_neg();
            let mut faces = 0u8;
            stack.push((seed_row, seed));
            while let Some((r, bits)) = stack.pop() {
                let bits = bits & open[r];
                if bits == 0 {
                    continue;
                }
                let run = spread(bits, open[r]);
                open[r] &= !run;
                let (ry, rz) = (r / SECTION_SIZE, r % SECTION_SIZE);
                faces |= touched(ry, rz, run);
                if ry > 0 {
                    stack.push((r - SECTION_SIZE, run));
                }
                if ry < last {
                    stack.push((r + SECTION_SIZE, run));
                }
                if rz > 0 {
                    stack.push((r - 1, run));
                }
                if rz < last {
                    stack.push((r + 1, run));
                }
            }
            for a in Face::ALL {
                for b in Face::ALL {
                    if faces & (1 << a as u8) != 0 && faces & (1 << b as u8) != 0 {
                        visibility.connect(a, b);
                    }
                }
            }
        }
    }
    visibility
}

#[cfg(test)]
mod tests;
