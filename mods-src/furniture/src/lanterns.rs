//! Lanterns are two single-cell rows: `furniture:lantern` (standing, item-linked) and
//! `furniture:lantern_hanging`, sharing one custom shape, the chain-row pattern. Block identity
//! picks which one, so bakes orient off the block id alone and placement needs no per-cell state.
//!
//! The rows differ in row data: the hanging one declares `behavior: "fragile"` and
//! `support: "above"`, so the engine breaks it the tick after its ceiling goes, and it drops the
//! standing lantern's item. The chain geometry is added here.
//!
//! The placement rule only reads `inputs.normal`. A world read would need answering by both the
//! server and the client (where the ghost comes from), and the client can't make sim host calls,
//! so a pure rule is the only thing both sides can agree on.
//!
//! Support checking is left to the host. Custom shape placement runs the row's own support gate
//! (`World::placement_support_ok`) on both sides, which enforces the standing row's `roots_face`,
//! the hanging row's `support: "above"` and each wall row's `support: "<side>"`. The mod picks
//! the row and the engine decides if it's allowed there. Nothing here says what counts as solid.

use mod_sdk::*;

use crate::keys;

pub(super) struct Lanterns {
    pub(super) shape: u16,
    pub(super) standing: BlockId,
    pub(super) hanging: BlockId,
    pub(super) wall: [BlockId; 4],
}

pub(super) const WALL_SIDES: [([i32; 3], &str); 4] = [
    ([0, 0, -1], keys::LANTERN_WALL_NORTH),
    ([0, 0, 1], keys::LANTERN_WALL_SOUTH),
    ([-1, 0, 0], keys::LANTERN_WALL_WEST),
    ([1, 0, 0], keys::LANTERN_WALL_EAST),
];

const T: f32 = 1.0 / 16.0;

const fn bx(min: [f32; 3], max: [f32; 3]) -> ShapeAabb {
    ShapeAabb {
        min: [min[0] * T, min[1] * T, min[2] * T],
        max: [max[0] * T, max[1] * T, max[2] * T],
    }
}

/// The lantern body, same for both rows.
///
/// Both sit at the same height on purpose, so they can share one tile set. A tile is an elevation
/// of the whole cell, so raising the hanging lantern even a texel would need a second side tile
/// just for the vertical shift.
///
/// Foot and hood are both 8 wide over a 6-wide body. The overhang is what reads as a lamp instead
/// of a box, and gives the side elevation three bands to shade.
const BODY: [ShapeAabb; 4] = [
    bx([4.0, 0.0, 4.0], [12.0, 1.0, 12.0]),
    bx([5.0, 1.0, 5.0], [11.0, 7.0, 11.0]),
    bx([4.0, 7.0, 4.0], [12.0, 9.0, 12.0]),
    bx([7.0, 9.0, 7.0], [9.0, 10.0, 9.0]),
];

fn bail(y0: f32, y1: f32) -> Vec<ShapeAabb> {
    super::chains::cell_links()
        .into_iter()
        .filter_map(|b| {
            let (lo, hi) = (b.min[1].max(y0 * T), b.max[1].min(y1 * T));
            (hi > lo).then_some(ShapeAabb {
                min: [b.min[0], lo, b.min[2]],
                max: [b.max[0], hi, b.max[2]],
            })
        })
        .collect()
}

/// Wall bracket, authored for the west wall (x = 0), turned onto the other three sides by
/// [`turn_to_side`]. Beam over the lantern, corbel under it at the wall, short bail down to the
/// finial.
///
/// Beam goes to x = 10 because the bail is the chain's own ring (6.5..9.5 wide, centred on axis 8).
/// Beam has to reach past it or the ring's far leg hangs in open air. Lamp still hangs plumb, axis
/// under the beam, not under its tip.
const BRACKET_WEST: [ShapeAabb; 2] = [
    bx([0.0, 12.0, 7.0], [10.0, 14.0, 9.0]),
    bx([0.0, 10.0, 7.0], [3.0, 12.0, 9.0]),
];

fn turn_to_side(b: ShapeAabb, d: [i32; 3]) -> ShapeAabb {
    let flip = |lo: f32, hi: f32| (1.0 - hi, 1.0 - lo);
    let (x0, x1, z0, z1) = (b.min[0], b.max[0], b.min[2], b.max[2]);
    let (x0, x1, z0, z1) = match d {
        [-1, 0, 0] => (x0, x1, z0, z1),
        [1, 0, 0] => {
            let (x0, x1) = flip(x0, x1);
            (x0, x1, z0, z1)
        }
        [0, 0, -1] => (z0, z1, x0, x1),
        _ => {
            let (z0, z1) = flip(x0, x1);
            (b.min[2], b.max[2], z0, z1)
        }
    };
    ShapeAabb {
        min: [x0, b.min[1], z0],
        max: [x1, b.max[1], z1],
    }
}

impl Lanterns {
    pub(super) fn row_for_normal(&self, n: [i32; 3]) -> BlockId {
        if n[1] < 0 {
            return self.hanging;
        }
        if n[1] > 0 {
            return self.standing;
        }
        let support = [-n[0], 0, -n[2]];
        match WALL_SIDES.iter().position(|(d, _)| *d == support) {
            Some(i) => self.wall[i],
            None => self.standing,
        }
    }

    pub(super) fn boxes_for(&self, block: BlockId) -> Vec<ShapeAabb> {
        let mut out = BODY.to_vec();
        if block == self.hanging {
            out.extend(bail(10.0, 16.0));
        }
        if let Some(i) = self.wall.iter().position(|&r| r == block) {
            let d = WALL_SIDES[i].0;
            out.extend(BRACKET_WEST.iter().map(|&b| turn_to_side(b, d)));
            out.extend(bail(10.0, 12.0));
        }
        out
    }

    pub(super) fn item_boxes(&self) -> Vec<ShapeAabb> {
        BODY.to_vec()
    }
}

pub(super) fn resolve_lanterns() -> Option<Lanterns> {
    let mut wall = [BlockId(0); 4];
    for (i, (_, row)) in WALL_SIDES.iter().enumerate() {
        wall[i] = resolve_block(row)?;
    }
    Some(Lanterns {
        shape: resolve_shape(keys::LANTERN_SHAPE)?,
        standing: resolve_block(keys::LANTERN)?,
        hanging: resolve_block(keys::LANTERN_HANGING)?,
        wall,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fam() -> Lanterns {
        Lanterns {
            shape: 0,
            standing: BlockId(1),
            hanging: BlockId(2),
            wall: [BlockId(3), BlockId(4), BlockId(5), BlockId(6)],
        }
    }

    #[test]
    fn the_clicked_face_picks_the_row() {
        let l = fam();
        assert_eq!(l.row_for_normal([0, -1, 0]), l.hanging);
        assert_eq!(l.row_for_normal([0, 1, 0]), l.standing);
        for (i, (support, _)) in WALL_SIDES.iter().enumerate() {
            let normal = [-support[0], 0, -support[2]];
            assert_eq!(l.row_for_normal(normal), l.wall[i], "normal {normal:?}");
        }
    }

    #[test]
    fn each_wall_rows_bracket_reaches_its_own_wall() {
        let l = fam();
        for (i, (d, _)) in WALL_SIDES.iter().enumerate() {
            let axis = if d[0] != 0 { 0 } else { 2 };
            let want = if d[axis] < 0 { 0.0 } else { 1.0 };
            let boxes = l.boxes_for(l.wall[i]);
            assert!(
                boxes.iter().any(|b| if want == 0.0 {
                    b.min[axis]
                } else {
                    b.max[axis]
                } == want),
                "wall row {i} has nothing against its {d:?} wall"
            );
        }
    }

    #[test]
    fn the_hanging_rows_bail_spans_finial_to_ceiling() {
        let l = fam();
        let standing = l.boxes_for(l.standing);
        let hanging = l.boxes_for(l.hanging);
        assert_eq!(&hanging[..standing.len()], &standing[..]);
        let added = &hanging[standing.len()..];
        assert!(!added.is_empty(), "the hanging row adds a bail");
        assert!(
            added.iter().any(|b| b.max[1] == 1.0),
            "the bail must reach the ceiling"
        );
        assert!(
            added.iter().all(|b| b.min[1] >= BODY[3].max[1]),
            "no bail box may reach below the finial it hooks over"
        );
    }

    #[test]
    fn every_box_stays_inside_the_cell() {
        let l = fam();
        let mut rows = vec![l.standing, l.hanging];
        rows.extend_from_slice(&l.wall);
        for block in rows {
            for b in l.boxes_for(block) {
                for a in 0..3 {
                    assert!(b.min[a] >= 0.0 && b.max[a] <= 1.0, "{b:?}");
                    assert!(b.min[a] < b.max[a], "{b:?}");
                }
            }
        }
    }
}
