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
//! bytes are ids, so those boundaries stay generic — a future family with id
//! bytes works without touching them.

use serde::{Deserialize, Serialize};

use crate::mathh::IVec3;

use super::super::{Aabb, Block, ShapeRenderBox};

/// Widest per-cell shape state, in bytes. The engine's own families need at
/// most four (a bbmodel cell's footprint offset plus its facing); the cap
/// keeps [`ShapeState`] `Copy` and cheap to pass through the mesher's hot
/// neighbour reads.
pub const SHAPE_STATE_MAX: usize = 4;

/// A cell's per-cell shape state as OPAQUE BYTES — meaningful only to the
/// family that owns the cell's block. This exact value is what the unified
/// section store holds, the save record persists, and the replication delta
/// ships, so `serde` here IS the wire encoding.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShapeState {
    len: u8,
    /// Bit `i` set = `bytes[i]` is a BLOCK-ID reference. Opaque to every
    /// reader except the save palette and the net transport, which rewrite
    /// masked bytes through their id mappings ([`remap_ids`](Self::remap_ids)).
    id_mask: u8,
    bytes: [u8; SHAPE_STATE_MAX],
}

impl ShapeState {
    /// No state (the cell's family is stateless, or the cell is not loaded).
    pub const NONE: ShapeState = ShapeState {
        len: 0,
        id_mask: 0,
        bytes: [0; SHAPE_STATE_MAX],
    };

    /// State from `bytes`, truncated at [`SHAPE_STATE_MAX`].
    #[inline]
    pub fn new(bytes: &[u8]) -> Self {
        Self::with_ids(bytes, 0)
    }

    /// State from `bytes` where the bits of `id_mask` flag which bytes are
    /// BLOCK-ID references (rewritten at the save/net boundaries).
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

    /// Which bytes are block-id references (bit `i` = byte `i`).
    #[inline]
    pub fn id_mask(&self) -> u8 {
        self.id_mask
    }

    /// Byte `i`, or `0` when the state is shorter — so a family decoding a
    /// missing/foreign state gets its zero value instead of panicking.
    #[inline]
    pub fn byte(&self, i: usize) -> u8 {
        if i < self.len as usize {
            self.bytes[i]
        } else {
            0
        }
    }

    /// Overwrite byte `i` in place, growing the state to cover it (new bytes
    /// between the old end and `i` read as zero — the same value they decode
    /// as when absent). The read-modify-write primitive a multi-field state
    /// (a model cell's offset + facing) edits through.
    #[inline]
    pub fn set_byte(&mut self, i: usize, v: u8) {
        debug_assert!(i < SHAPE_STATE_MAX);
        if i >= self.len as usize {
            self.len = (i + 1) as u8;
        }
        self.bytes[i] = v;
    }

    /// Rewrite every id-masked byte through `f` — the save palette / net
    /// transport boundary hook. Non-masked bytes are untouched.
    #[inline]
    pub fn remap_ids(&mut self, f: impl Fn(u8) -> u8) {
        let mut mask = self.id_mask;
        while mask != 0 {
            let i = mask.trailing_zeros() as usize;
            mask &= mask - 1;
            if i < self.len as usize {
                self.bytes[i] = f(self.bytes[i]);
            }
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// A typed VIEW over a cell's opaque state bytes. Implemented NEXT TO the
/// type it decodes (the owner's module — `StairState` in the stair's home,
/// `DoorState` in the door's), never in engine code: the byte layout is the
/// owner's private vocabulary, and this trait is the only bridge.
///
/// `owns` is the read gate: a foreign cell's bytes must never decode through
/// another owner's view, so the generic accessors answer the type's default
/// semantics (`from_cell(NONE)`) for any block the view does not own.
pub trait CellView: Sized {
    /// Which blocks own state this view may decode.
    fn owns(block: Block) -> bool;
    /// Decode the stored bytes. Absence ([`ShapeState::NONE`]) MUST decode to
    /// the type's default semantics — never panic, never garbage.
    fn from_cell(state: ShapeState) -> Self;
}

/// The writable half of a cell-state vocabulary. A pure VIEW (a stair's
/// refined corner, decoded from bytes another writer maintains) implements
/// [`CellView`] only.
pub trait CellCodec: CellView {
    /// Encode to stored bytes. May return [`ShapeState::NONE`] for a default
    /// value the store elides (a vertical log, an empty slab stack).
    fn to_cell(&self) -> ShapeState;
}

/// The world reads a shape family may perform — and the ONLY ones it has.
///
/// Implemented by the sim world and by the mesher's padded section snapshot.
/// Reads outside the caller's knowledge (an unloaded cell, a neighbour beyond
/// the mesh pad) answer with air / [`ShapeState::NONE`] / `None`, never a
/// panic: a family must degrade to "nothing there", which is exactly how the
/// per-family producers already behaved at streaming edges.
pub trait ShapeNeighborhood {
    /// The block at `pos` (air when unknown or unloaded).
    fn block(&self, pos: IVec3) -> Block;

    /// The per-cell shape state at `pos`, opaque to everyone but the family
    /// owning that cell's block.
    fn shape_state(&self, pos: IVec3) -> ShapeState;

    /// A Layer-3 bake's drawn boxes for `pos`, when one is cached and
    /// reachable. `None` covers "never baked", "trapped bake", and "outside
    /// the caller's window" alike — the family then falls back to its static
    /// form, the established Layer-3 failure policy.
    fn baked(&self, _pos: IVec3) -> Option<&[ShapeRenderBox]> {
        None
    }

    /// A Layer-3 SIM bake's authoritative collision boxes for `pos`, when one
    /// is cached and reachable — the sim twin of [`baked`](Self::baked)
    /// (collision is interned, render is not, so the two caches stay
    /// separate). Same `None` semantics: the family falls back to the row's
    /// static collision. The mesher's pad keeps the default — it never
    /// resolves collision.
    fn baked_collision(&self, _pos: IVec3) -> Option<&'static [Aabb]> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The opaque-state contract: round-trips within the cap, truncates past
    /// it, and reads short states as zeros rather than panicking (a family
    /// decoding a foreign or missing state must degrade, not crash).
    #[test]
    fn shape_state_round_trips_truncates_and_reads_short_as_zero() {
        let s = ShapeState::new(&[7, 9]);
        assert_eq!(s.bytes(), &[7, 9]);
        assert_eq!((s.byte(0), s.byte(1)), (7, 9));
        assert_eq!(s.byte(2), 0, "past the end reads as zero");
        assert_eq!(ShapeState::NONE.byte(0), 0);
        assert!(ShapeState::NONE.is_empty());

        let over = ShapeState::new(&[1, 2, 3, 4, 5, 6]);
        assert_eq!(
            over.bytes().len(),
            SHAPE_STATE_MAX,
            "over-cap state truncates instead of overflowing"
        );
    }
}
