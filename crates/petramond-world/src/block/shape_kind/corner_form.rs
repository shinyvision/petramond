//! Corner-form box algebra: turning an authored box list, and composing two
//! turned lists into the OUTER (intersection) and INNER (union) corner forms.
//!
//! Pure geometry over [`BoxDef`] — no world, no registry, no JSON.

use super::*;

/// Where each face lands after ONE quarter turn about Y: entry `i` of the
/// turned box takes the authored box's face `FACE_BEFORE_TURN[i]`. The turn is
/// `(x, z) -> (1 - z, x)`, so the authored `-Z` front comes to `+X`, matching
/// [`Facing`](crate::facing::Facing)'s North → East step.
pub const FACE_BEFORE_TURN: [usize; 6] = [5, 4, 2, 3, 0, 1];

/// Where the AUTHORED front face (`-Z`, canonical index 5) sits after `turns`
/// quarter turns — how a row's `front` tile finds its face. The draw indexes
/// this by the face's TOTAL turn (the shape's plus the face's own
/// [`BoxDef::art_turns`]), which is what lets a corner form's front art wrap
/// around two faces that are different numbers of turns from the authored one.
///
/// The item cube needs no such lookup for the straight form: it draws at two
/// turns, which lands the front on `+Z`, exactly the index
/// `block_icon_faces_with_state` already writes the front tile to.
pub const FRONT_AFTER_TURN: [usize; 4] = [5, 0, 4, 1];

/// The [`ShapeFace::uv_turns`](crate::block::ShapeFace::uv_turns) a box face
/// needs once its shape is turned `turns` quarter turns about Y.
///
/// The four SIDE faces need none — a quarter turn carries a side face's
/// cell-local UV to the next direction unchanged. `+Y`/`-Y` sample a fixed
/// tile through a turned footprint, so their art must be turned back, and
/// opposite ways because the two mappings are mirror images. Shared by the
/// chunk mesh and the item cube so a turned shape textures the same in the
/// world, the hand and the icon.
pub fn face_uv_turns(face: usize, turns: u8) -> u8 {
    match face {
        2 => turns & 3,
        3 => (4 - (turns & 3)) & 3,
        _ => 0,
    }
}

impl BoxDef {
    /// This box turned one quarter about the cell's Y centre.
    pub fn turned(&self) -> BoxDef {
        let (min, max) = (self.aabb.min, self.aabb.max);
        BoxDef {
            aabb: Aabb {
                min: [1.0 - max[2], min[1], min[0]],
                max: [1.0 - min[2], max[1], max[0]],
            },
            faces: std::array::from_fn(|i| self.faces[FACE_BEFORE_TURN[i]]),
            tiles: std::array::from_fn(|i| self.tiles[FACE_BEFORE_TURN[i]]),
            occludes: self.occludes,
            collides: self.collides,
            double_sided: self.double_sided,
            art_turns: std::array::from_fn(|i| self.art_turns[FACE_BEFORE_TURN[i]]),
        }
    }

    /// This box with every face's art frame advanced `turns` quarter turns —
    /// what a corner form's donor list needs so its inherited faces still know
    /// which frame they were authored in (see [`art_turns`](Self::art_turns)).
    fn art_advanced(&self, turns: u8) -> BoxDef {
        BoxDef {
            art_turns: self.art_turns.map(|t| (t + turns) & 3),
            ..*self
        }
    }
}

/// Turn a whole box list by `turns` quarter turns.
pub(super) fn turned_list(list: &[BoxDef], turns: u8) -> Vec<BoxDef> {
    let mut v: Vec<BoxDef> = list.to_vec();
    for _ in 0..(turns & 3) {
        v = v.iter().map(BoxDef::turned).collect();
    }
    v
}

/// The quarter-turned donor list a corner form composes against: turned
/// geometry whose faces also REMEMBER they were authored one turn round.
pub(super) fn donor_list(list: &[BoxDef], turns: u8) -> Vec<BoxDef> {
    turned_list(list, turns)
        .iter()
        .map(|b| b.art_advanced(turns))
        .collect()
}

/// The INTERSECTION of two box lists — the OUTER corner form, exactly the
/// stair rule's `back_mask & back_mask` lifted from quadrant masks to boxes:
/// what remains is the matter both perpendicular orientations agree on, so the
/// front treatment wraps around the turned side. Each result face inherits its
/// style from the parent whose face plane it lies on (`self` preferred where
/// both are coplanar — the top of a full-cell slab), including that parent's
/// [`art_turns`](BoxDef::art_turns), which is how the turned parent's FRONT
/// tile and UV frame reach the wrapped face.
pub(super) fn intersect_lists(a: &[BoxDef], b: &[BoxDef]) -> Vec<BoxDef> {
    let mut out = Vec::new();
    for pa in a {
        for pb in b {
            let mut r = pa.aabb;
            for ax in 0..3 {
                r.min[ax] = r.min[ax].max(pb.aabb.min[ax]);
                r.max[ax] = r.max[ax].min(pb.aabb.max[ax]);
            }
            if (0..3).any(|ax| r.min[ax] >= r.max[ax]) {
                continue;
            }
            // Face i of the result lies on pa's plane, pb's plane, or strictly
            // inside one parent (then the OTHER parent's plane bounds it).
            let mut piece = BoxDef { aabb: r, ..*pa };
            for i in 0..6 {
                let (axis, high) = [
                    (0, true),
                    (0, false),
                    (1, true),
                    (1, false),
                    (2, true),
                    (2, false),
                ][i];
                let plane = if high { r.max[axis] } else { r.min[axis] };
                let of = |p: &BoxDef| {
                    if high {
                        p.aabb.max[axis] == plane
                    } else {
                        p.aabb.min[axis] == plane
                    }
                };
                let parent = if of(pa) { pa } else { pb };
                piece.faces[i] = parent.faces[i];
                piece.tiles[i] = parent.tiles[i];
                piece.art_turns[i] = parent.art_turns[i];
            }
            piece.occludes = pa.occludes && pb.occludes;
            piece.collides = pa.collides && pb.collides;
            piece.double_sided = pa.double_sided || pb.double_sided;
            if !out.contains(&piece) {
                out.push(piece);
            }
        }
    }
    out
}

/// The UNION of two box lists — the INNER corner form (`back_mask | back_mask`):
/// simply both lists, `self` first so the coincident-plane tie-break keeps the
/// straight parent's faces wherever the two overlap exactly (interpenetration
/// is the box vocabulary's normal state; buried faces are harmless overdraw).
/// Exact duplicates are dropped.
pub(super) fn union_lists(a: &[BoxDef], b: &[BoxDef]) -> Vec<BoxDef> {
    let mut out: Vec<BoxDef> = a.to_vec();
    for pb in b {
        if !out.iter().any(|pa| pa.aabb == pb.aabb) {
            out.push(*pb);
        }
    }
    out
}

/// The union of every box's extent — a form's selection outline and target box.
pub(super) fn union_bounds(set: &[BoxDef]) -> Aabb {
    let mut bounds = Aabb {
        min: [f32::INFINITY; 3],
        max: [f32::NEG_INFINITY; 3],
    };
    for b in set {
        for a in 0..3 {
            bounds.min[a] = bounds.min[a].min(b.aabb.min[a]);
            bounds.max[a] = bounds.max[a].max(b.aabb.max[a]);
        }
    }
    bounds
}
