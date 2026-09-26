//! The trapdoor: a thin panel lying flat across a cell that swings up onto one
//! of its vertical edges, shared by collision, selection, and rendering so they
//! can never disagree.
//!
//! A trapdoor is the door's horizontal sibling and carries the same three bits:
//! - `facing` — the edge the panel is HINGED on, i.e. the edge nearest the
//!   placer and the edge it stands on once open.
//! - `open` — swings the panel 90° off the floor (or ceiling) onto that edge.
//! - `top` — the closed panel lies against the cell's CEILING rather than its
//!   floor (clicking the underside of a block, or the rotation key).
//!
//! The cell-local collision/selection boxes are returned as `'static` slices so
//! `World::collision_boxes_at` can hand them straight to the swept-AABB
//! collider. The drawn panel is the `petramond:trapdoor` animated model
//! (`assets/animated_models.json`): the same closed slab swung about a hinge
//! half a thickness in from the cell edge on BOTH axes, which is what lands the
//! swung panel exactly on the open collision slab — `crate::animated_model`'s
//! tests hold the two to each other.

use crate::block::Aabb;
use crate::door::THICKNESS;
use crate::facing::Facing;

/// The near edge of the panel's thin axis (`1 - THICKNESS`).
const FAR: f32 = 1.0 - THICKNESS;

/// A placed trapdoor cell's state, packed into one byte in the cell-state store.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub struct TrapdoorState {
    /// The hinged edge — nearest the placer when placed, and the edge the open
    /// panel stands on.
    pub facing: Facing,
    /// Swung 90° up (or down, from the ceiling) onto the hinged edge.
    pub open: bool,
    /// The closed panel lies against the cell's ceiling rather than its floor.
    pub top: bool,
}

impl crate::block::CellView for Option<TrapdoorState> {
    fn owns(block: crate::block::Block) -> bool {
        crate::block::shape_kind_families::is_trapdoor(block)
    }
    /// `None` when no state is stored (the length distinguishes it from the
    /// valid all-zero pose byte) — readers then fall back to the row's static
    /// form, the same failure policy as the door.
    fn from_cell(s: crate::block::ShapeState) -> Self {
        if s.is_empty() {
            return None;
        }
        Some(TrapdoorState::decode(s.byte(0)))
    }
}

impl crate::block::CellView for TrapdoorState {
    fn owns(block: crate::block::Block) -> bool {
        crate::block::shape_kind_families::is_trapdoor(block)
    }
    fn from_cell(s: crate::block::ShapeState) -> Self {
        TrapdoorState::decode(s.byte(0))
    }
}
impl crate::block::CellCodec for TrapdoorState {
    fn to_cell(&self) -> crate::block::ShapeState {
        crate::block::ShapeState::new(&[self.encode()])
    }
}

impl TrapdoorState {
    /// Pack into a byte for the cell-state store + save codec: bits 0..2 =
    /// facing, bit 2 = open, bit 3 = top.
    #[inline]
    pub fn encode(self) -> u8 {
        self.facing.to_u8() | ((self.open as u8) << 2) | ((self.top as u8) << 3)
    }

    /// Inverse of [`encode`](Self::encode). Unknown facing bits fall back to North.
    #[inline]
    pub fn decode(b: u8) -> TrapdoorState {
        TrapdoorState {
            facing: Facing::from_u8(b & 0b11),
            open: (b & 0b100) != 0,
            top: (b & 0b1000) != 0,
        }
    }
}

/// One thin slab box in cell-local coords (`0..1`), flat or on a vertical edge.
macro_rules! slab {
    (y, $lo:expr, $hi:expr) => {
        &[Aabb {
            min: [0.0, $lo, 0.0],
            max: [1.0, $hi, 1.0],
        }]
    };
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

/// Closed: a flat panel on the cell's floor or ceiling.
const CLOSED_FLOOR: &[Aabb] = slab!(y, 0.0, THICKNESS);
const CLOSED_CEILING: &[Aabb] = slab!(y, FAR, 1.0);
/// Open: the panel stands full-height on the edge it is hinged to.
const OPEN_NORTH: &[Aabb] = slab!(z, 0.0, THICKNESS); // -Z edge
const OPEN_SOUTH: &[Aabb] = slab!(z, FAR, 1.0); // +Z edge
const OPEN_WEST: &[Aabb] = slab!(x, 0.0, THICKNESS); // -X edge
const OPEN_EAST: &[Aabb] = slab!(x, FAR, 1.0); // +X edge

/// The cell-local collision boxes for a trapdoor cell in `state` — a flat panel
/// on the floor/ceiling, or a full-height slab on the hinged edge when `open`.
#[inline]
pub fn collision_boxes(state: TrapdoorState) -> &'static [Aabb] {
    if !state.open {
        return if state.top {
            CLOSED_CEILING
        } else {
            CLOSED_FLOOR
        };
    }
    match state.facing {
        Facing::North => OPEN_NORTH,
        Facing::South => OPEN_SOUTH,
        Facing::West => OPEN_WEST,
        Facing::East => OPEN_EAST,
    }
}

/// The selection / raycast-target box for a trapdoor cell — the single slab of
/// its [`collision_boxes`], so the outline + break overlay hug the panel.
#[inline]
pub fn selection_aabb(state: TrapdoorState) -> ([f32; 3], [f32; 3]) {
    let b = collision_boxes(state)[0];
    (b.min, b.max)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FACINGS: [Facing; 4] = [Facing::North, Facing::South, Facing::West, Facing::East];

    #[test]
    fn state_byte_round_trips() {
        for &facing in &FACINGS {
            for &open in &[false, true] {
                for &top in &[false, true] {
                    let s = TrapdoorState { facing, open, top };
                    assert_eq!(TrapdoorState::decode(s.encode()), s);
                }
            }
        }
    }

    #[test]
    fn an_open_panel_stands_on_the_edge_it_is_hinged_to() {
        // The open slab must be the thin one on `facing`'s side, whichever half
        // the panel rests in — a closed panel is thin on Y, an open one is not.
        for &facing in &FACINGS {
            for &top in &[false, true] {
                let open = collision_boxes(TrapdoorState {
                    facing,
                    open: true,
                    top,
                })[0];
                assert!(
                    (open.max[1] - open.min[1] - 1.0).abs() < 1e-5,
                    "{facing:?}: an open panel is full height"
                );
                let (axis, near) = match facing {
                    Facing::North => (2, true),
                    Facing::South => (2, false),
                    Facing::West => (0, true),
                    Facing::East => (0, false),
                };
                let (lo, hi) = (open.min[axis], open.max[axis]);
                assert!(hi - lo < 0.5, "{facing:?}: the open panel is thin");
                assert!(
                    if near { lo < 1e-5 } else { hi > 1.0 - 1e-5 },
                    "{facing:?}: the open panel stands on its hinged edge"
                );
            }
        }
    }
}
