//! Wooden-door geometry + state, shared by collision, selection, and rendering so
//! they can never disagree.
//!
//! A door is a 2-tall thin slab standing on a cell edge. Its dynamic state lives in
//! the chunk door map (see [`Chunk`](crate::chunk::Chunk) / `world::door`):
//! - `facing` — the outward normal of the **closed** slab, i.e. the edge nearest the
//!   placer. The placer faces a block and the door appears on the near edge: facing
//!   north ⇒ the door sits on the **south** edge ⇒ `facing == South` (this matches
//!   `facing_from_forward` used by furnaces/chests).
//! - `open` — swings the slab 90° onto the adjacent edge of the SAME cell. The hinge is
//!   on the placing player's **left** corner; facing south ⇒ opens onto the west edge.
//! - `top` — which of the two stacked cells this is (the upper half).
//!
//! The per-cell collision/selection boxes (full cell height, a thin slab on one edge)
//! are returned here as `'static` slices so `World::collision_boxes_at` can hand
//! them straight to the swept-AABB collider. The drawn door is the
//! `petramond:door` animated model (`assets/animated_models.json`): the same closed
//! slab swung about a hinge inset half a thickness from the cell corner, so the
//! swung slab lands exactly on the open collision slab (the door stays within its
//! own cell, not 3px into the neighbour) — `crate::animated_model`'s tests hold the
//! two to each other.

use crate::block::Aabb;
use crate::facing::Facing;

/// Slab thickness as a fraction of a cell — 3/16
pub const THICKNESS: f32 = 3.0 / 16.0;
/// The near edge of the slab's thin axis (`1 - THICKNESS`).
const FAR: f32 = 1.0 - THICKNESS;

/// A placed door cell's state, packed into one byte in the chunk door map.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub struct DoorState {
    /// Outward normal of the CLOSED slab — the edge it rests on (nearest the placer).
    pub facing: Facing,
    /// Swung 90° onto the adjacent (hinge-right) edge when `true`.
    pub open: bool,
    /// The upper of the two stacked door cells when `true`.
    pub top: bool,
}

impl crate::block::CellView for Option<DoorState> {
    fn owns(block: crate::block::Block) -> bool {
        crate::block::shape_kind_families::is_door(block)
    }
    /// `None` when no state is stored (the length distinguishes it from the
    /// valid all-zero pose byte) — readers then fall back to the row's static
    /// form, the door's failure policy.
    fn from_cell(s: crate::block::ShapeState) -> Self {
        if s.is_empty() {
            return None;
        }
        Some(DoorState::decode(s.byte(0)))
    }
}

impl crate::block::CellView for DoorState {
    fn owns(block: crate::block::Block) -> bool {
        crate::block::shape_kind_families::is_door(block)
    }
    fn from_cell(s: crate::block::ShapeState) -> Self {
        DoorState::decode(s.byte(0))
    }
}
impl crate::block::CellCodec for DoorState {
    fn to_cell(&self) -> crate::block::ShapeState {
        crate::block::ShapeState::new(&[self.encode()])
    }
}

impl DoorState {
    /// Pack into a byte for the chunk door map + save codec: bits 0..2 = facing,
    /// bit 2 = open, bit 3 = top.
    #[inline]
    pub fn encode(self) -> u8 {
        self.facing.to_u8() | ((self.open as u8) << 2) | ((self.top as u8) << 3)
    }

    /// Inverse of [`encode`](Self::encode). Unknown facing bits fall back to North.
    #[inline]
    pub fn decode(b: u8) -> DoorState {
        DoorState {
            facing: Facing::from_u8(b & 0b11),
            open: (b & 0b100) != 0,
            top: (b & 0b1000) != 0,
        }
    }
}

/// One full-height thin slab box on edge `*`, in cell-local coords (`0..1`).
macro_rules! slab {
    (z, $lo:expr, $hi:expr) => {
        &[Aabb {
            min: [0.0, 0.0, $lo],
            max: [1.0, 1.0, $hi],
        }]
    };
    (x, $lo:expr, $hi:expr) => {
        &[Aabb {
            min: [$lo, 0.0, 0.0],
            max: [$hi, 1.0, 1.0],
        }]
    };
}

// Closed slabs sit on the `facing` edge; opening swings them 90° (hinge on the
// placer's LEFT corner) onto the adjacent edge, staying within THIS cell. Worked out
// per facing (see module docs); the open edge is the one the drawn swing lands on
// (verified against the animated model in `crate::animated_model`'s tests).
const NORTH_CLOSED: &[Aabb] = slab!(z, 0.0, THICKNESS); // -Z edge
const NORTH_OPEN: &[Aabb] = slab!(x, FAR, 1.0); //  +X edge
const SOUTH_CLOSED: &[Aabb] = slab!(z, FAR, 1.0); // +Z edge
const SOUTH_OPEN: &[Aabb] = slab!(x, 0.0, THICKNESS); //  -X edge
const WEST_CLOSED: &[Aabb] = slab!(x, 0.0, THICKNESS); // -X edge
const WEST_OPEN: &[Aabb] = slab!(z, 0.0, THICKNESS); //  -Z edge
const EAST_CLOSED: &[Aabb] = slab!(x, FAR, 1.0); // +X edge
const EAST_OPEN: &[Aabb] = slab!(z, FAR, 1.0); //  +Z edge

/// The cell-local collision boxes for a door cell in `state` — one thin full-height
/// slab on the closed edge, or on the swung-open edge when `open`.
#[inline]
pub fn collision_boxes(state: DoorState) -> &'static [Aabb] {
    match (state.facing, state.open) {
        (Facing::North, false) => NORTH_CLOSED,
        (Facing::North, true) => NORTH_OPEN,
        (Facing::South, false) => SOUTH_CLOSED,
        (Facing::South, true) => SOUTH_OPEN,
        (Facing::West, false) => WEST_CLOSED,
        (Facing::West, true) => WEST_OPEN,
        (Facing::East, false) => EAST_CLOSED,
        (Facing::East, true) => EAST_OPEN,
    }
}

/// The selection / raycast-target box for a door cell — the union (a single slab) of
/// its [`collision_boxes`], so the outline + break overlay hug the actual panel.
#[inline]
pub fn selection_aabb(state: DoorState) -> ([f32; 3], [f32; 3]) {
    let b = collision_boxes(state)[0];
    (b.min, b.max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_byte_round_trips() {
        for &facing in &[Facing::North, Facing::South, Facing::West, Facing::East] {
            for &open in &[false, true] {
                for &top in &[false, true] {
                    let s = DoorState { facing, open, top };
                    assert_eq!(DoorState::decode(s.encode()), s);
                }
            }
        }
    }

    #[test]
    fn opening_swaps_the_collision_slab_to_the_adjacent_edge() {
        // The closed slab is thin on its facing axis; the open slab is thin on the
        // perpendicular axis (the door has swung onto the adjacent edge).
        let thin_axis = |b: Aabb| {
            let dx = b.max[0] - b.min[0];
            let dz = b.max[2] - b.min[2];
            if dx < dz {
                0
            } else {
                2
            }
        };
        for &facing in &[Facing::North, Facing::South, Facing::West, Facing::East] {
            let closed = collision_boxes(DoorState {
                facing,
                open: false,
                top: false,
            })[0];
            let open = collision_boxes(DoorState {
                facing,
                open: true,
                top: false,
            })[0];
            assert_ne!(
                thin_axis(closed),
                thin_axis(open),
                "{facing:?}: opening must rotate the slab onto the perpendicular edge"
            );
        }
    }
}
