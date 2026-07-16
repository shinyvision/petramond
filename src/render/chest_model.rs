//! World-space + item-space geometry for chests: an inset six-sided body box, a lid
//! box that hinges open, and a small protruding front latch. Baked each frame into a
//! reusable dynamic vbuf/ibuf and drawn by the **existing** opaque block pipeline (no
//! new pipeline) — exactly like [`item_entity`](super::item_entity).
//!
//! The chest is modelled in a unit block (0..1 per axis) with its **front on `+Z`**
//! (so the same geometry serves the placed chest AND the isometric inventory icon /
//! held item / dropped chest, which present the `+Z` face). For a placed chest the
//! lid verts are rotated about the rear-bottom hinge edge by the current open angle
//! (a CPU vertex transform, like the item-entity spin), then the whole model is
//! rotated about its vertical centre to the block's `facing` and translated to the
//! world. Verts carry the chest's sampled skylight.
//!
//! Why dynamic (not chunk-meshed): the lid angle changes every frame while a chest is
//! open, and re-meshing the owning chunk per frame would be far too expensive, so the
//! chest opts out of chunk meshing entirely (see `mesh::builder`) and is drawn here.

use glam::Vec3;

use super::item_cube::{orient_faces_to_block, push_box_faces_lit};
use super::ChestInstance;
use crate::atlas::Tile;
use crate::mesh::Vertex;

/// Inset (m) of the body/lid from the block edges — the chest is 14/16 wide & deep.
const INSET: f32 = 1.0 / 16.0;
/// Body box: rests on the floor, 14×10×14 (1/16..15/16 horizontally, 0..10/16 tall).
const BODY_MIN: Vec3 = Vec3::new(INSET, 0.0, INSET);
const BODY_MAX: Vec3 = Vec3::new(1.0 - INSET, 10.0 / 16.0, 1.0 - INSET);
/// Lid box: atop the body, meeting it exactly at the seam (10/16..14/16 tall) with
/// NO overlap, so the lid and body side faces never share a plane (which would
/// z-fight). The seam faces (body top / lid bottom) are coplanar but face opposite
/// ways and are mutually occluded when closed.
const LID_MIN: Vec3 = Vec3::new(INSET, 10.0 / 16.0, INSET);
const LID_MAX: Vec3 = Vec3::new(1.0 - INSET, 14.0 / 16.0, 1.0 - INSET);
/// Latch box: a small knob protruding from the front (`+Z`) centre, straddling the
/// seam (8/16..12/16 tall, centred on the hinge) and 1/16 proud of the front face.
/// It hinges WITH the lid (see `push_chest_world`).
const LATCH_MIN: Vec3 = Vec3::new(7.0 / 16.0, 8.0 / 16.0, 14.0 / 16.0);
const LATCH_MAX: Vec3 = Vec3::new(9.0 / 16.0, 12.0 / 16.0, 1.0);
/// Hinge edge (model space): the lid's rear-bottom edge at the seam. Front is `+Z`,
/// so the back is `-Z` (z = 1/16) and the seam is y = 10/16. The lid (+ latch) rotate
/// about the X axis through this edge so the front edge swings up and back on opening.
const HINGE_Y: f32 = 10.0 / 16.0;
const HINGE_Z: f32 = INSET;
/// Lid rotation at fully open (radians): ~90° so the lid stands up at the back. The
/// front is `+Z`, so opening is a negative rotation about X (front edge lifts toward
/// `-Z`).
const LID_OPEN_RADIANS: f32 = -std::f32::consts::FRAC_PI_2;
/// Vertical lift (model units) applied to the item-space chest so its 14/16-tall body
/// sits centred in the icon's unit cube instead of bottom-aligned.
const ITEM_LIFT: f32 = (1.0 - 14.0 / 16.0) * 0.5;

/// Per-face tiles (`ALL_FACES` order: PosX, NegX, PosY, NegY, PosZ, NegZ) for the
/// body box. Front (`+Z`) carries the chest-front art; the top is the interior (seen
/// when the lid opens; hidden by the lid when closed).
fn body_faces() -> [Tile; 6] {
    let e = crate::atlas::engine();
    [
        e.chest_side,   // PosX
        e.chest_side,   // NegX
        e.chest_inside, // PosY (covered by the lid when closed; interior when open)
        e.chest_side,   // NegY (bottom, unseen)
        e.chest_front,  // PosZ (front)
        e.chest_side,   // NegZ (back)
    ]
}
/// Per-face tiles for the lid box. Top is the chest's visible top; the underside is
/// the interior, seen when the lid opens.
fn lid_faces() -> [Tile; 6] {
    let e = crate::atlas::engine();
    [
        e.chest_lid_side,  // PosX
        e.chest_lid_side,  // NegX
        e.chest_top,       // PosY (the chest's top surface)
        e.chest_inside,    // NegY (lid underside / interior)
        e.chest_lid_front, // PosZ (front)
        e.chest_lid_side,  // NegZ (back)
    ]
}
/// The latch knob is small; the metallic latch tile reads fine on every face.
fn latch_faces() -> [Tile; 6] {
    [crate::atlas::engine().chest_latch; 6]
}

/// Bake all `instances` into `verts`/`indices` (cleared first, capacity reused) and
/// return the index count. The caller frustum-culls instances before calling.
pub fn build_chests(
    instances: &[ChestInstance],
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
) -> u32 {
    verts.clear();
    indices.clear();
    for inst in instances {
        push_chest_world(verts, indices, inst);
    }
    indices.len() as u32
}

/// Append one placed chest (body + hinged lid + latch) for `inst`, lit by its
/// skylight, oriented to its `facing` at the world block `pos`.
fn push_chest_world(verts: &mut Vec<Vertex>, indices: &mut Vec<u32>, inst: &ChestInstance) {
    let sky = super::lighting::DynLight::new(inst.skylight, inst.blocklight);
    let start = verts.len();
    push_box_faces_lit(verts, indices, body_faces(), BODY_MIN, BODY_MAX, sky);

    // Lid + latch hinge together about the rear-bottom seam edge: append both, then
    // rotate that range by the open angle so the front edge swings up and back.
    let lid_start = verts.len();
    push_box_faces_lit(verts, indices, lid_faces(), LID_MIN, LID_MAX, sky);
    push_box_faces_lit(verts, indices, latch_faces(), LATCH_MIN, LATCH_MAX, sky);
    let angle = inst.lid01.clamp(0.0, 1.0) * LID_OPEN_RADIANS;
    if angle != 0.0 {
        let (s, c) = angle.sin_cos();
        for v in verts[lid_start..].iter_mut() {
            let [x, y, z] = v.pos;
            let dy = y - HINGE_Y;
            let dz = z - HINGE_Z;
            v.pos = [x, HINGE_Y + dy * c - dz * s, HINGE_Z + dy * s + dz * c];
        }
    }

    // Orient the whole model to `facing` and translate to the world block origin.
    orient_faces_to_block(verts, start, inst.facing, inst.pos);
}

/// Build a CLOSED chest (inset body + latch + lid) centred in the cube
/// `[origin, origin+size]`, front on `+Z`, lit by `skylight`. The item-space
/// counterpart of `block_model::push_cube_faces_lit` for the chest — used by the
/// inventory icon, the held item, and dropped chests so they read as a 3D chest
/// rather than a plain cube. The model is lifted so it sits centred in the cube.
pub(super) fn push_chest_item(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    origin: Vec3,
    size: f32,
    light: super::lighting::DynLight,
) {
    let lift = Vec3::new(0.0, ITEM_LIFT, 0.0);
    let map = |a: Vec3| origin + (a + lift) * size;
    // Painter order: the inventory icon is drawn in the DEPTHLESS UI pass, so paint
    // back-to-front — body, then the lid over it, then the latch in front (otherwise
    // the lid would overpaint the latch).
    push_box_faces_lit(
        verts,
        indices,
        body_faces(),
        map(BODY_MIN),
        map(BODY_MAX),
        light,
    );
    push_box_faces_lit(
        verts,
        indices,
        lid_faces(),
        map(LID_MIN),
        map(LID_MAX),
        light,
    );
    push_box_faces_lit(
        verts,
        indices,
        latch_faces(),
        map(LATCH_MIN),
        map(LATCH_MAX),
        light,
    );
}

/// Full-bright [`push_chest_item`] for the inventory icon (which is unlit, like the
/// `block_model::push_block_item_cube` icons).
pub(super) fn push_chest_item_full(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    origin: Vec3,
    size: f32,
) {
    push_chest_item(
        verts,
        indices,
        origin,
        size,
        super::lighting::DynLight::FULL,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facing::Facing;

    fn inst(facing: Facing, lid01: f32) -> ChestInstance {
        ChestInstance {
            pos: Vec3::new(10.0, 64.0, -5.0),
            facing,
            lid01,
            skylight: super::super::lighting::FULL_SKYLIGHT,
            blocklight: 0,
        }
    }

    #[test]
    fn one_chest_bakes_three_boxes() {
        let mut v = Vec::new();
        let mut i = Vec::new();
        let n = build_chests(
            std::slice::from_ref(&inst(Facing::North, 0.0)),
            &mut v,
            &mut i,
        );
        assert_eq!(v.len(), 72, "body + latch + lid = 3 boxes × 24 verts");
        assert_eq!(n, 108, "3 boxes × 36 indices");
    }

    #[test]
    fn empty_input_produces_no_geometry() {
        let mut v = Vec::new();
        let mut i = Vec::new();
        assert_eq!(build_chests(&[], &mut v, &mut i), 0);
        assert!(v.is_empty() && i.is_empty());
    }

    #[test]
    fn closed_chest_fits_within_its_block_footprint() {
        let mut v = Vec::new();
        let mut i = Vec::new();
        build_chests(
            std::slice::from_ref(&inst(Facing::North, 0.0)),
            &mut v,
            &mut i,
        );
        for vert in &v {
            let [x, y, z] = vert.pos;
            assert!((10.0..=11.0).contains(&x), "x within cell, got {x}");
            assert!((-5.0..=-4.0).contains(&z), "z within cell, got {z}");
            assert!((64.0..=65.0).contains(&y), "y within cell, got {y}");
        }
    }

    #[test]
    fn opening_the_lid_raises_geometry_above_the_block() {
        let mut closed_v = Vec::new();
        let mut closed_i = Vec::new();
        build_chests(
            std::slice::from_ref(&inst(Facing::North, 0.0)),
            &mut closed_v,
            &mut closed_i,
        );
        let closed_top = closed_v.iter().map(|v| v.pos[1]).fold(f32::MIN, f32::max);

        let mut open_v = Vec::new();
        let mut open_i = Vec::new();
        build_chests(
            std::slice::from_ref(&inst(Facing::North, 1.0)),
            &mut open_v,
            &mut open_i,
        );
        let open_top = open_v.iter().map(|v| v.pos[1]).fold(f32::MIN, f32::max);

        // The open lid swings up well above the closed chest's top.
        assert!(
            open_top > closed_top + 0.3,
            "lid should rise when opening: {closed_top} -> {open_top}"
        );
    }
}
