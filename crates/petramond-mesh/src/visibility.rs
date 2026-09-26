//! Section face-to-face connectivity, the input of the renderer's occlusion
//! culling (a visibility graph over sections, flooded from the camera).
//!
//! Two faces of a section CONNECT when a path of non-occluding cells joins a
//! cell on one to a cell on the other. A straight sight line crossing the
//! section in through one face and out through another passes only
//! non-occluding cells, so it can only cross between connected faces — which
//! is what lets the renderer skip sections no sight line from the camera can
//! reach. The graph is computed on the mesh worker with the rest of the mesh.

use std::sync::LazyLock;

use petramond_world::block::{Block, MeshEmitter};
use petramond_world::chunk::{section_idx, SECTION_SIZE, SECTION_VOLUME};
use petramond_world::section::Section;

use crate::face::Face;

/// Which pairs of a section's six faces see each other through it: a
/// symmetric 6×6 bit matrix, bit `a * 6 + b` for faces `a` and `b` in
/// [`Face::ALL`] order.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SectionVisibility(u64);

/// Bits of the full 6×6 matrix.
const ALL_BITS: u64 = (1 << 36) - 1;

impl SectionVisibility {
    /// Every face sees every other: air, and the conservative answer for a
    /// section nothing is known about.
    pub const ALL: Self = Self(ALL_BITS);
    /// No face sees any other: solid rock.
    pub const NONE: Self = Self(0);

    #[inline]
    fn bit(a: Face, b: Face) -> u64 {
        1 << (a as u32 * 6 + b as u32)
    }

    /// Whether a sight line entering through `a` can leave through `b`.
    #[inline]
    pub fn connects(self, a: Face, b: Face) -> bool {
        self.0 & Self::bit(a, b) != 0
    }

    fn connect(&mut self, a: Face, b: Face) {
        self.0 |= Self::bit(a, b) | Self::bit(b, a);
    }

    /// Connectivity joining exactly the listed face pairs (both ways).
    pub fn from_pairs(pairs: &[(Face, Face)]) -> Self {
        let mut visibility = Self::NONE;
        for &(a, b) in pairs {
            visibility.connect(a, b);
        }
        visibility
    }

    /// The connectivity of `section`'s cells.
    pub fn of_section(section: &Section) -> Self {
        if section.is_empty_air() {
            return Self::ALL;
        }
        if section.all_opaque() {
            return Self::NONE;
        }
        let occluders = occluders();
        let mut open = [0u64; SECTION_VOLUME / 64];
        let mut any_closed = false;
        for (i, word) in open.iter_mut().enumerate() {
            for bit in 0..64 {
                let id = section.block_at_idx(i * 64 + bit);
                if occluders.get(id as usize).copied().unwrap_or(false) {
                    any_closed = true;
                } else {
                    *word |= 1 << bit;
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
    /// Unknown connectivity is full connectivity: culling on it can only
    /// draw too much, never hide a visible section.
    fn default() -> Self {
        Self::ALL
    }
}

/// Whether each block id occludes sight completely: an opaque full cube.
/// Anything thinner, see-through or fluid lets some line through.
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

/// The section-local cell coordinates of a [`section_idx`] index.
#[inline]
fn coords(i: usize) -> (usize, usize, usize) {
    (
        i % SECTION_SIZE,
        i / (SECTION_SIZE * SECTION_SIZE),
        (i / SECTION_SIZE) % SECTION_SIZE,
    )
}

/// The faces of the section a cell touches.
fn touched(x: usize, y: usize, z: usize) -> u8 {
    let last = SECTION_SIZE - 1;
    let mut faces = 0u8;
    for (on, face) in [
        (x == last, Face::PosX),
        (x == 0, Face::NegX),
        (y == last, Face::PosY),
        (y == 0, Face::NegY),
        (z == last, Face::PosZ),
        (z == 0, Face::NegZ),
    ] {
        if on {
            faces |= 1 << face as u8;
        }
    }
    faces
}

/// Flood every open region that reaches the section boundary and connect
/// the faces each touches.
fn flood(mut open: [u64; SECTION_VOLUME / 64]) -> SectionVisibility {
    let mut visibility = SectionVisibility::NONE;
    let mut stack: Vec<usize> = Vec::with_capacity(SECTION_VOLUME);
    let take = |open: &mut [u64; SECTION_VOLUME / 64], i: usize| -> bool {
        let (word, bit) = (i / 64, 1u64 << (i % 64));
        let was = open[word] & bit != 0;
        open[word] &= !bit;
        was
    };
    for seed in 0..SECTION_VOLUME {
        let (x, y, z) = coords(seed);
        if touched(x, y, z) == 0 || !take(&mut open, seed) {
            continue;
        }
        let mut faces = 0u8;
        stack.push(seed);
        while let Some(i) = stack.pop() {
            let (x, y, z) = coords(i);
            faces |= touched(x, y, z);
            let last = SECTION_SIZE - 1;
            let mut visit = |nx: usize, ny: usize, nz: usize| {
                let n = section_idx(nx, ny, nz);
                if take(&mut open, n) {
                    stack.push(n);
                }
            };
            if x < last {
                visit(x + 1, y, z);
            }
            if x > 0 {
                visit(x - 1, y, z);
            }
            if y < last {
                visit(x, y + 1, z);
            }
            if y > 0 {
                visit(x, y - 1, z);
            }
            if z < last {
                visit(x, y, z + 1);
            }
            if z > 0 {
                visit(x, y, z - 1);
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
    visibility
}

#[cfg(test)]
mod tests;
