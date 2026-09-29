//! The PRIMITIVE seam a shape family reads the world through.
//!
//! A family resolving its boxes needs two things about a cell: which block is
//! there, and what per-cell state that block carries. Nothing else. Expressing
//! exactly that as a trait is what lets one family implementation serve every
//! caller — because the callers do NOT share a world type:
//!
//! - the sim/main thread holds a `&World` (server and client replica alike);
//! - the chunk mesher runs on a WORKER thread over a padded section snapshot
//!   (`SectionMeshPad`) and has no `&World` at all.
//!
//! That split is the whole reason the engine grew six independent "give me
//! this cell's boxes" producers. With this seam a family is written once and
//! resolves identically wherever it runs, because it cannot reach past it.
//!
//! # State is opaque
//!
//! [`ShapeState`] is BYTES — and it is THE per-cell block-state currency: the
//! unified section store, the save record, and the replication delta all carry
//! exactly this value, and only the family/behavior that owns the cell's
//! block decodes it. A stair family reads a facing and a half out of byte 0 —
//! the engine never knows that. This is what makes a family independent:
//! adding one introduces no engine-side vocabulary, no save-format change,
//! and no wire-format change.
//!
//! The ONE thing the engine must see inside the bytes is BLOCK-ID references
//! (a slab's two layer materials): the save palette and the net transport
//! rewrite ids at their boundaries. [`ShapeState::id_mask`] declares which
//! bytes START an id, so those boundaries stay generic — a future family with
//! id bytes works without touching them. A block id is TWO bytes, so a set
//! mask bit claims `bytes[i]` and `bytes[i + 1]` as one little-endian id;
//! [`ShapeState::id_bytes`] and [`ShapeState::id_at`] are the pair.

use serde::{Deserialize, Serialize};

use crate::mathh::IVec3;

use super::super::{Aabb, Block, ShapeRenderBox};

pub const SHAPE_STATE_MAX: usize = 8;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ShapeState {
    len: u8,
    /// Bit `i` set means `bytes[i..i + 2]` is a little-endian block-id reference. Only the save
    /// palette and the net transport read these, rewriting masked ids through their mappings
    /// ([`remap_ids`](Self::remap_ids)). Every other reader treats them as opaque.
    id_mask: u8,
    bytes: [u8; SHAPE_STATE_MAX],
}

impl ShapeState {
    pub const NONE: ShapeState = ShapeState {
        len: 0,
        id_mask: 0,
        bytes: [0; SHAPE_STATE_MAX],
    };

    #[inline]
    pub fn new(bytes: &[u8]) -> Self {
        Self::with_ids(bytes, 0)
    }

    #[inline]
    pub fn with_ids(bytes: &[u8], id_mask: u8) -> Self {
        let len = bytes.len().min(SHAPE_STATE_MAX);
        let mut out = ShapeState {
            len: len as u8,
            id_mask,
            bytes: [0; SHAPE_STATE_MAX],
        };
        out.bytes[..len].copy_from_slice(&bytes[..len]);
        out
    }

    #[inline]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    #[inline]
    pub fn id_mask(&self) -> u8 {
        self.id_mask
    }

    #[inline]
    pub fn id_bytes(id: u16) -> [u8; 2] {
        id.to_le_bytes()
    }

    #[inline]
    pub fn id_at(&self, i: usize) -> u16 {
        u16::from_le_bytes([self.byte(i), self.byte(i + 1)])
    }

    #[inline]
    pub fn byte(&self, i: usize) -> u8 {
        if i < self.len as usize {
            self.bytes[i]
        } else {
            0
        }
    }

    #[inline]
    pub fn remap_ids(&mut self, f: impl Fn(u16) -> u16) {
        let mut mask = self.id_mask;
        while mask != 0 {
            let i = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            if i + 1 < self.len as usize {
                let [lo, hi] = f(self.id_at(i)).to_le_bytes();
                self.bytes[i] = lo;
                self.bytes[i + 1] = hi;
            }
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

pub trait CellView: Sized {
    fn owns(block: Block) -> bool;
    fn from_cell(state: ShapeState) -> Self;
}

pub trait CellCodec: CellView {
    fn to_cell(&self) -> ShapeState;
}

pub trait ShapeNeighborhood {
    fn block(&self, pos: IVec3) -> Block;

    fn shape_state(&self, pos: IVec3) -> ShapeState;

    fn baked(&self, _pos: IVec3) -> Option<&[ShapeRenderBox]> {
        None
    }

    fn baked_collision(&self, _pos: IVec3) -> Option<&'static [Aabb]> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_state_round_trips_truncates_and_reads_short_as_zero() {
        let s = ShapeState::new(&[7, 9]);
        assert_eq!(s.bytes(), &[7, 9]);
        assert_eq!((s.byte(0), s.byte(1)), (7, 9));
        assert_eq!(s.byte(2), 0, "past the end reads as zero");
        assert_eq!(ShapeState::NONE.byte(0), 0);
        assert!(ShapeState::NONE.is_empty());

        let over = ShapeState::new(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        assert_eq!(
            over.bytes().len(),
            SHAPE_STATE_MAX,
            "over-cap state truncates instead of overflowing"
        );
    }

    #[test]
    fn id_masked_pairs_carry_and_remap_the_full_block_id() {
        let [lo, hi] = ShapeState::id_bytes(300);
        let mut s = ShapeState::with_ids(&[0b0111, lo, hi], 0b010);
        assert_eq!(s.id_at(1), 300);
        s.remap_ids(|id| id + 1000);
        assert_eq!(s.id_at(1), 1300);
        assert_eq!(s.byte(0), 0b0111, "non-masked bytes are untouched");
    }
}
