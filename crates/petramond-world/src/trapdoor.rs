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
//! collider. The rendered panel (`render::trapdoor_model`) builds the same
//! closed slab and rotates it about [`hinge_pivot`] by [`swing_radians`] —
//! pivoting half a thickness in from the cell edge on BOTH axes, which is what
//! lands the swung panel exactly on the open collision slab.

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
        block.shape_family() == crate::block::ShapeFamily::Trapdoor
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
        block.shape_family() == crate::block::ShapeFamily::Trapdoor
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

/// The cell-local `(y, z)` pivot the rendered panel swings about, in the
/// canonical south-hinged frame — the hinged edge's inner corner, **inset by
/// half the panel thickness on both axes**. The inset is what makes a rigid 90°
/// swing land the panel exactly on the open [`collision_boxes`] slab instead of
/// a thickness outside the cell (the same correction the door's hinge makes).
#[inline]
pub fn hinge_pivot(top: bool) -> (f32, f32) {
    let i = THICKNESS / 2.0;
    (if top { 1.0 - i } else { i }, 1.0 - i)
}

/// The swing angle (radians, in the canonical frame's `(y, z)` plane) for a
/// panel `open01` of the way open. A floor panel lifts one way, a ceiling panel
/// drops the other; both land standing on the hinged edge.
#[inline]
pub fn swing_radians(top: bool, open01: f32) -> f32 {
    let sign = if top { -1.0 } else { 1.0 };
    sign * open01.clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2
}

/// Rotate cell-local point `(y, z)` about the horizontal line through
/// `(hy, hz)` by `angle`. Shared by `render::trapdoor_model` so the drawn swing
/// pivots on the hinge.
#[inline]
pub fn rotate_about(y: f32, z: f32, hy: f32, hz: f32, angle: f32) -> (f32, f32) {
    let (s, c) = angle.sin_cos();
    let (dy, dz) = (y - hy, z - hz);
    (hy + dy * c - dz * s, hz + dy * s + dz * c)
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
    fn the_rendered_swing_lands_on_the_open_collision_slab_in_cell() {
        // THE geometric claim: rotating the closed panel a full 90° about the
        // (doubly inset) hinge pivot reproduces the OPEN collision slab — so
        // the open panel stands on its own cell's hinged edge and never pokes
        // into the neighbour. Canonical frame: hinged on +Z (South).
        let profile = |b: Aabb| ([b.min[1], b.min[2]], [b.max[1], b.max[2]]);
        for &top in &[false, true] {
            let closed = collision_boxes(TrapdoorState {
                facing: Facing::South,
                open: false,
                top,
            })[0];
            let open = collision_boxes(TrapdoorState {
                facing: Facing::South,
                open: true,
                top,
            })[0];
            let (hy, hz) = hinge_pivot(top);
            let angle = swing_radians(top, 1.0);
            let ([cy0, cz0], [cy1, cz1]) = profile(closed);
            let p0 = rotate_about(cy0, cz0, hy, hz, angle);
            let p1 = rotate_about(cy1, cz1, hy, hz, angle);
            let rmin = [p0.0.min(p1.0), p0.1.min(p1.1)];
            let rmax = [p0.0.max(p1.0), p0.1.max(p1.1)];
            let (omin, omax) = profile(open);
            for k in 0..2 {
                assert!(
                    (rmin[k] - omin[k]).abs() < 1e-5 && (rmax[k] - omax[k]).abs() < 1e-5,
                    "top={top}: swung closed {rmin:?}..{rmax:?} != open {omin:?}..{omax:?}"
                );
            }
            assert!(
                rmin[0] >= -1e-5
                    && rmax[0] <= 1.0 + 1e-5
                    && rmin[1] >= -1e-5
                    && rmax[1] <= 1.0 + 1e-5,
                "top={top}: open slab {rmin:?}..{rmax:?} escaped the cell"
            );
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
