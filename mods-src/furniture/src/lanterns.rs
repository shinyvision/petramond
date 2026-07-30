//! Lanterns: two single-cell rows — `furniture:lantern` (standing, the
//! item-linked base) and `furniture:lantern_hanging` — sharing ONE custom
//! shape, the chain-row pattern. Which one a click writes is block IDENTITY,
//! so the bakes orient from the block id alone and placement needs no
//! per-cell state.
//!
//! The two differ in exactly two ways and both are ROW data, not geometry:
//! the hanging row declares `behavior: "fragile"` + `support: "above"`, so the
//! ENGINE breaks it the tick after its ceiling goes, and it drops the standing
//! lantern's item. The geometry difference is the CHAIN, added here.
//!
//! The placement rule reads nothing but `inputs.normal` — the chain's rule. A
//! world read would have to be answered by both the server AND the client
//! instance (which is where the ghost comes from), and the client cannot make
//! sim host calls at all, so the pure rule is the only one the two sides can
//! agree on by construction.
//!
//! It does not have to answer "is there really something there", because the
//! HOST does: a custom shape's placement now runs the row's own support gate
//! (`World::placement_support_ok`) on both sides, so the standing row's
//! `roots_face: "solid_face"`, the hanging row's `support: "above"` and each
//! wall row's `support: "<side>"` are all enforced by the engine against its
//! own world. The mod picks the row; the engine decides whether that row may
//! be there. Nothing here enumerates what counts as solid, which is the point.

use mod_sdk::*;

/// The lantern family: the shared shape-kind id and its rows — standing,
/// hanging, and one per WALL side (indexed by [`WALL_SIDES`]).
pub(super) struct Lanterns {
    pub(super) shape: u8,
    pub(super) standing: BlockId,
    pub(super) hanging: BlockId,
    pub(super) wall: [BlockId; 4],
}

/// The wall rows, in the order their block ids are held, keyed by the
/// DIRECTION OF THE SUPPORT from the lantern — the same thing the row's
/// `support` field declares, so the row name, the field and this table cannot
/// drift apart.
pub(super) const WALL_SIDES: [([i32; 3], &str); 4] = [
    ([0, 0, -1], "north"),
    ([0, 0, 1], "south"),
    ([-1, 0, 0], "west"),
    ([1, 0, 0], "east"),
];

const T: f32 = 1.0 / 16.0;

/// One box in texel coordinates (`0..=16`), the unit every other shape in this
/// pack is authored in.
const fn bx(min: [f32; 3], max: [f32; 3]) -> ShapeAabb {
    ShapeAabb {
        min: [min[0] * T, min[1] * T, min[2] * T],
        max: [max[0] * T, max[1] * T, max[2] * T],
    }
}

/// The lantern BODY, identical for both rows.
///
/// Both rows sit at the same height on purpose: it is what lets the two share
/// ONE tile set. A tile is an elevation of the whole cell, so raising the
/// hanging lantern by even one texel would have meant a second side tile whose
/// only difference is a vertical shift.
///
/// A foot and a hood both 8 wide over a 6-wide body — the overhang is what
/// reads as a lamp rather than a box, and it gives the side elevation three
/// distinct bands to shade.
const BODY: [ShapeAabb; 4] = [
    bx([4.0, 0.0, 4.0], [12.0, 1.0, 12.0]), // foot
    bx([5.0, 1.0, 5.0], [11.0, 7.0, 11.0]), // glazed body
    bx([4.0, 7.0, 4.0], [12.0, 9.0, 12.0]), // hood
    bx([7.0, 9.0, 7.0], [9.0, 10.0, 9.0]),  // finial the bail grips
];

/// The bail: a real cell of `furniture:chain`, CLIPPED to `y0..y1`.
///
/// Taken from the chain rather than restated, so the two can never drift. A
/// hanging lantern is normally strung under a chain block, and any divergence
/// shows as a step right at the cell boundary — the first cut had its own
/// plates and hand-drawn links, and the run visibly changed width, pitch and
/// palette all at once. Retuning the chain now retunes the bail with it.
///
/// CLIPPED, not scaled: the chain's own cell pattern is what a chain block
/// directly above would be showing at these heights, so the run reads through
/// the boundary unbroken. What falls out at the lamp's end is most of one ring
/// with its bottom bar cut away — a link hooked over the finial, which is
/// exactly what the joint is.
///
/// The tile art matches for the same reason: `gen_lantern.py` blits
/// `chain_link`'s own texels into the lantern's side tile.
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

/// The wall BRACKET, authored for the west wall (`x = 0`) and turned onto the
/// other three sides by [`turn_to_side`]. A beam out over the lantern, a
/// corbel under it at the wall, and a short bail from the beam down to the
/// lantern's finial.
///
/// The beam runs to `x = 10`: the bail is the chain's own ring (6.5..9.5 wide,
/// centred on the lamp's axis at 8), so the beam must reach past it or the
/// ring's far leg hangs off the beam's end into open air. The lamp still hangs
/// plumb — its axis is under the beam, not under the beam's tip.
const BRACKET_WEST: [ShapeAabb; 2] = [
    bx([0.0, 12.0, 7.0], [10.0, 14.0, 9.0]), // beam
    bx([0.0, 10.0, 7.0], [3.0, 12.0, 9.0]),  // corbel
];

/// Turn a west-wall box onto the side whose support direction is `d`.
///
/// The lantern is symmetric about both horizontal axes, so a mirror and an axis
/// swap are all four sides need — no rotation matrix, and every coordinate
/// stays on the texel grid the tiles are authored against.
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
    /// The row a clicked face's normal writes: the UNDERSIDE of a block hangs
    /// one, a SIDE face brackets one off that wall, and the top of a block (or
    /// anything else) stands one.
    ///
    /// A wall row is keyed by where its SUPPORT is, which is the opposite of
    /// the face normal: clicking a block's east face puts the lantern east of
    /// it, so the wall is to the lantern's west.
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

    /// The box list for a placed lantern cell, from its block id alone (a pure
    /// function of the cell, per the bake purity rule).
    pub(super) fn boxes_for(&self, block: BlockId) -> Vec<ShapeAabb> {
        let mut out = BODY.to_vec();
        if block == self.hanging {
            out.extend(bail(10.0, 16.0));
        }
        if let Some(i) = self.wall.iter().position(|&r| r == block) {
            let d = WALL_SIDES[i].0;
            out.extend(BRACKET_WEST.iter().map(|&b| turn_to_side(b, d)));
            // The bail is already on the lamp's own axis, so it does not turn.
            out.extend(bail(10.0, 12.0));
        }
        out
    }

    /// The item form is always the STANDING lantern — a held lantern reads as
    /// the lamp, not as a lamp with a length of chain hanging off it.
    pub(super) fn item_boxes(&self) -> Vec<ShapeAabb> {
        BODY.to_vec()
    }
}

/// Resolve the lantern family at init (registry-only calls, legal on any
/// instance — the bakes and the placement plan run on both). `None` when the
/// pack content didn't load, in which case lanterns fall back to the row's
/// static shape and the rest of the mod keeps working.
pub(super) fn resolve_lanterns() -> Option<Lanterns> {
    let mut wall = [BlockId(0); 4];
    for (i, (_, name)) in WALL_SIDES.iter().enumerate() {
        wall[i] = resolve_block(&format!("furniture:lantern_wall_{name}"))?;
    }
    Some(Lanterns {
        shape: resolve_shape("furniture:lantern")?,
        standing: resolve_block("furniture:lantern")?,
        hanging: resolve_block("furniture:lantern_hanging")?,
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
        // A side face brackets off the wall OPPOSITE the normal: clicking a
        // block's east face leaves the wall to the lantern's west.
        for (i, (support, _)) in WALL_SIDES.iter().enumerate() {
            let normal = [-support[0], 0, -support[2]];
            assert_eq!(l.row_for_normal(normal), l.wall[i], "normal {normal:?}");
        }
    }

    /// Every wall row's bracket must actually TOUCH its own wall, or the lamp
    /// brackets off thin air while the engine's support rule still holds it up
    /// from a wall it does not visibly reach.
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

    /// The bail is the ONLY difference between the two rows' geometry, and it
    /// must span finial to ceiling — stopping short of the ceiling leaves the
    /// lamp visibly hanging off nothing, and reaching below the finial puts
    /// chain inside the lamp's own hood.
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

    /// Every box has to stay inside its own cell: placement is single-cell, so
    /// anything poking out would draw and collide where nothing was written.
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
