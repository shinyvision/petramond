//! World-space geometry for placed doors: a 2-tall thin slab built in a canonical
//! south-facing frame, swung about its hinge by the open angle, then oriented to the
//! door's `facing` and translated to the world. Baked each frame into a reusable
//! dynamic vbuf/ibuf and drawn by the **existing** opaque block pipeline (no new
//! pipeline) — exactly like [`chest_model`](super::chest_model), the precedent for an
//! animated (non-chunk-meshed) block.
//!
//! The door is modelled CLOSED on the `+Z` (south) edge, hinged on its SE corner; the
//! lower half is `y ∈ [0,1]` (textured `bottom_tile`), the upper `y ∈ [1,2]`
//! (`top_tile`). Opening rotates the slab about the hinge (see [`crate::door`]); the
//! whole model is then rotated about its vertical centre to the block's `facing` and
//! translated to the world, like the chest. The swung edge matches the door's
//! collision edge (a rigid swing's 3px body lands on the outer face of that edge, an
//! imperceptible offset from the in-cell collision slab — see `crate::door`).

use glam::Vec3;

use super::block_model::push_box_faces_lit_mirrored;
use super::DoorInstance;
use crate::door::{self, THICKNESS};
use crate::furnace::Facing;
use crate::mesh::{Vertex, UV_MODE_THIN_U, UV_MODE_THIN_V};

/// Canonical closed-door slab extent on the thin (Z) axis: flush with the `+Z` face.
const Z_BACK: f32 = 1.0 - THICKNESS;
const Z_FRONT: f32 = 1.0;

/// Bake all `instances` into `verts`/`indices` (cleared first, capacity reused) and
/// return the index count. The caller frustum-culls instances before calling.
pub fn build_doors(
    instances: &[DoorInstance],
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
) -> u32 {
    verts.clear();
    indices.clear();
    for inst in instances {
        push_door_world(verts, indices, inst);
    }
    indices.len() as u32
}

/// Append one placed door (lower + upper slab halves) for `inst`, swung by its angle,
/// oriented to its `facing`, lit by its skylight, at the world lower-cell `pos`.
fn push_door_world(verts: &mut Vec<Vertex>, indices: &mut Vec<u32>, inst: &DoorInstance) {
    let sky = inst.skylight;
    let start = verts.len();
    // Per-face tiles in `ALL_FACES` order [PosX, NegX, PosY, NegY, PosZ, NegZ]: the
    // canonical door's front + back are the ±Z faces (the wide faces the player sees),
    // which carry the door ART; the four thin edge faces (±X sides, ±Y top/bottom) carry
    // the distinct `side_tile` (the door's edge — a plank strip, NOT the stretched front
    // art). Built canonically (south, front = +Z); the rotations below carry it.
    let half_faces = |art: crate::atlas::Tile| {
        [
            inst.side_tile, // PosX (east edge)
            inst.side_tile, // NegX (west edge)
            inst.side_tile, // PosY (top edge)
            inst.side_tile, // NegY (bottom edge)
            art,            // PosZ (front, faces the placer)
            art,            // NegZ (back)
        ]
    };
    // Mirror ONLY the back face (NegZ, index 5) so the door reads identically (hinge on
    // the hinge side) from front AND back, instead of flipping handedness behind.
    const MIRROR_BACK: [bool; 6] = [false, false, false, false, false, true];
    // Thin-face UV mode (ALL_FACES order [PosX, NegX, PosY, NegY, PosZ, NegZ]): the four
    // 3/16-deep EDGE faces would squish a whole tile across that thin edge, so each crops
    // its tile to a matching strip. The side edges (±X) are thin along Z, which maps to
    // the face's U axis → crop-U (mode 1); the top/bottom edges (±Y) are thin along Z,
    // which maps to V → crop-V (mode 2). The wide front/back art (±Z) is full-tile (0).
    const SLICE: [u32; 6] = [
        UV_MODE_THIN_U,
        UV_MODE_THIN_U,
        UV_MODE_THIN_V,
        UV_MODE_THIN_V,
        0,
        0,
    ];
    // Canonical south-facing slab: lower half (bottom art) then upper half (top art).
    push_box_faces_lit_mirrored(
        verts,
        indices,
        half_faces(inst.bottom_tile),
        Vec3::new(0.0, 0.0, Z_BACK),
        Vec3::new(1.0, 1.0, Z_FRONT),
        sky,
        MIRROR_BACK,
        SLICE,
    );
    push_box_faces_lit_mirrored(
        verts,
        indices,
        half_faces(inst.top_tile),
        Vec3::new(0.0, 1.0, Z_BACK),
        Vec3::new(1.0, 2.0, Z_FRONT),
        sky,
        MIRROR_BACK,
        SLICE,
    );

    // Swing about the canonical hinge pivot (inset from the corner so the open slab
    // lands in THIS cell — see `door::hinge_pivot`) by the open angle.
    let angle = door::swing_radians(inst.open01);
    if angle != 0.0 {
        let (hx, hz) = door::hinge_pivot(Facing::South);
        for v in verts[start..].iter_mut() {
            let (x, z) = door::rotate_about(v.pos[0], v.pos[2], hx, hz, angle);
            v.pos[0] = x;
            v.pos[2] = z;
        }
    }

    // Orient the whole slab to `facing` (canonical = South) about the cell's vertical
    // centre, then translate to the world lower-cell origin. CPU vertex transform since
    // the opaque pipeline has no per-draw model matrix (same as chests/item entities).
    let (ys, yc) = facing_yaw(inst.facing).sin_cos();
    for v in verts[start..].iter_mut() {
        let [x, y, z] = v.pos;
        let dx = x - 0.5;
        let dz = z - 0.5;
        let rx = 0.5 + dx * yc + dz * ys;
        let rz = 0.5 - dx * ys + dz * yc;
        v.pos = [inst.pos.x + rx, inst.pos.y + y, inst.pos.z + rz];
    }
}

/// Yaw (radians) rotating the canonical closed edge (`+Z`, South) to `facing`'s edge —
/// the same convention as [`chest_model::facing_yaw`](super::chest_model).
fn facing_yaw(facing: Facing) -> f32 {
    use std::f32::consts::{FRAC_PI_2, PI};
    match facing {
        Facing::South => 0.0,
        Facing::North => PI,
        Facing::East => FRAC_PI_2,
        Facing::West => -FRAC_PI_2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::atlas::Tile;

    fn inst(facing: Facing, open01: f32) -> DoorInstance {
        DoorInstance {
            pos: Vec3::new(10.0, 64.0, -5.0),
            facing,
            open01,
            bottom_tile: Tile::named("oak_door_bottom"),
            top_tile: Tile::named("oak_door_top"),
            side_tile: Tile::named("oak_planks"),
            skylight: super::super::lighting::FULL_SKYLIGHT,
        }
    }

    #[test]
    fn one_door_bakes_two_boxes() {
        let mut v = Vec::new();
        let mut i = Vec::new();
        let n = build_doors(
            std::slice::from_ref(&inst(Facing::South, 0.0)),
            &mut v,
            &mut i,
        );
        assert_eq!(v.len(), 48, "lower + upper half = 2 boxes × 24 verts");
        assert_eq!(n, 72, "2 boxes × 36 indices");
    }

    #[test]
    fn empty_input_produces_no_geometry() {
        let mut v = Vec::new();
        let mut i = Vec::new();
        assert_eq!(build_doors(&[], &mut v, &mut i), 0);
        assert!(v.is_empty() && i.is_empty());
    }

    #[test]
    fn a_closed_door_spans_two_cells_tall_within_its_footprint() {
        let mut v = Vec::new();
        let mut i = Vec::new();
        build_doors(
            std::slice::from_ref(&inst(Facing::South, 0.0)),
            &mut v,
            &mut i,
        );
        let (mut ymin, mut ymax) = (f32::MAX, f32::MIN);
        for vert in &v {
            ymin = ymin.min(vert.pos[1]);
            ymax = ymax.max(vert.pos[1]);
            // Closed door stays within the lower cell's x/z column (south edge).
            assert!((10.0..=11.0).contains(&vert.pos[0]));
            assert!((-5.0..=-4.0).contains(&vert.pos[2]));
        }
        assert!((ymin - 64.0).abs() < 1e-4, "rests on the cell floor");
        assert!((ymax - 66.0).abs() < 1e-4, "two cells tall");
    }

    #[test]
    fn thin_edge_faces_carry_a_uv_slice_mode() {
        // The 3/16-deep edge faces must crop their tile (packed bits 29..32) so the
        // plank side isn't a whole tile squished flat; the wide front/back art is full.
        let mut v = Vec::new();
        let mut i = Vec::new();
        build_doors(
            std::slice::from_ref(&inst(Facing::South, 0.0)),
            &mut v,
            &mut i,
        );
        // Faces are emitted per box in ALL_FACES order [PosX, NegX, PosY, NegY, PosZ, NegZ],
        // 4 verts each; read the first vert of each face in the lower box.
        let slice = |face_idx: usize| (v[face_idx * 4].packed >> 29) & 0x3;
        assert_eq!(slice(0), 1, "PosX side edge crops U");
        assert_eq!(slice(1), 1, "NegX side edge crops U");
        assert_eq!(slice(2), 2, "PosY top edge crops V");
        assert_eq!(slice(3), 2, "NegY bottom edge crops V");
        assert_eq!(slice(4), 0, "PosZ front art is full-tile");
        assert_eq!(slice(5), 0, "NegZ back art is full-tile");
    }

    #[test]
    fn opening_sweeps_the_slab_off_its_closed_edge() {
        // A south-facing door closed lies on the +Z edge (z ≈ 1 within the cell); fully
        // open it has swung onto a perpendicular edge, so its z-extent shrinks toward an
        // edge while its x-extent collapses to the thin slab.
        let mut closed = (Vec::new(), Vec::new());
        build_doors(
            std::slice::from_ref(&inst(Facing::South, 0.0)),
            &mut closed.0,
            &mut closed.1,
        );
        let mut open = (Vec::new(), Vec::new());
        build_doors(
            std::slice::from_ref(&inst(Facing::South, 1.0)),
            &mut open.0,
            &mut open.1,
        );
        let span = |v: &[Vertex], axis: usize| {
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for vert in v {
                lo = lo.min(vert.pos[axis]);
                hi = hi.max(vert.pos[axis]);
            }
            hi - lo
        };
        // Closed: wide in x (full cell), thin in z. Open: thin in x, wide-ish in z.
        assert!(span(&closed.0, 0) > 0.9 && span(&closed.0, 2) < 0.3);
        assert!(span(&open.0, 0) < 0.3 && span(&open.0, 2) > 0.9);
    }
}
