//! World-space geometry for placed trapdoors: a thin panel built in a canonical
//! south-hinged frame, swung about its hinge by the open angle, then oriented to
//! the panel's `facing` and translated to the world. Baked each frame into the
//! same dynamic vbuf/ibuf as the doors and drawn by the **existing** opaque
//! block pipeline — see [`door_model`](super::door_model), whose conventions
//! (canonical south frame, mirrored back face, thin-edge UV slices) this
//! follows exactly.
//!
//! The panel is modelled CLOSED lying on the cell floor (or, for a `top` panel,
//! against its ceiling), hinged on the `+Z` (south) edge. Opening rotates it in
//! the `(y, z)` plane about the inset hinge (see [`petramond_world::trapdoor`])
//! until it stands on that edge; the whole panel is then rotated about the
//! cell's vertical centre to its `facing` and translated to the world.

use glam::Vec3;

use super::item_cube::{orient_faces_to_block, push_box_faces_lit_mirrored, thin_face_slice_modes};
use super::TrapdoorInstance;
use petramond_mesh::Vertex;
use petramond_world::door::THICKNESS;
use petramond_world::trapdoor;

/// Append every instance's geometry to `verts`/`indices` (NOT cleared — the
/// panels share one stream with the doors). The caller frustum-culls first.
pub fn push_trapdoors(
    instances: &[TrapdoorInstance],
    render_origin: glam::IVec3,
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
) {
    for inst in instances {
        push_trapdoor_world(verts, indices, inst, render_origin);
    }
}

/// Append one placed trapdoor for `inst`, swung by its angle, oriented to its
/// `facing`, lit by its cell light, at the world cell `pos`.
fn push_trapdoor_world(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    inst: &TrapdoorInstance,
    render_origin: glam::IVec3,
) {
    let light = super::lighting::DynLight::new(inst.skylight, inst.blocklight);
    let start = verts.len();
    // Per-face tiles in `ALL_FACES` order [PosX, NegX, PosY, NegY, PosZ, NegZ]:
    // the panel's wide faces are the ±Y ones, which carry the trapdoor ART; the
    // four thin edge faces carry the distinct `side_tile` (a plank strip).
    let faces = [
        inst.side_tile,   // PosX edge
        inst.side_tile,   // NegX edge
        inst.top_tile,    // PosY — the face walked on
        inst.bottom_tile, // NegY — the underside
        inst.side_tile,   // PosZ edge (the hinged one)
        inst.side_tile,   // NegZ edge
    ];
    // Mirror the underside so the art reads the same handedness from below as
    // from above — the door's back-face rule, one axis over.
    const MIRROR_UNDER: [bool; 6] = [false, false, false, true, false, false];
    let (y0, y1) = if inst.top {
        (1.0 - THICKNESS, 1.0)
    } else {
        (0.0, THICKNESS)
    };
    // The four 3/16-deep edge faces crop their tile to a matching strip rather
    // than squishing a whole one flat; the wide ±Y art stays full-tile.
    let (min, max) = (Vec3::new(0.0, y0, 0.0), Vec3::new(1.0, y1, 1.0));
    let slice = thin_face_slice_modes(min, max);
    push_box_faces_lit_mirrored(verts, indices, faces, min, max, light, MIRROR_UNDER, slice);

    // Swing about the canonical hinge pivot (inset from the cell edge on both
    // axes so the open panel lands in THIS cell) by the open angle.
    let angle = trapdoor::swing_radians(inst.top, inst.open01);
    if angle != 0.0 {
        let (hy, hz) = trapdoor::hinge_pivot(inst.top);
        for v in verts[start..].iter_mut() {
            let (y, z) = trapdoor::rotate_about(v.pos[1], v.pos[2], hy, hz, angle);
            v.pos[1] = y;
            v.pos[2] = z;
        }
    }

    orient_faces_to_block(
        verts,
        start,
        inst.facing,
        (inst.pos - render_origin).as_vec3(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use petramond_math::facing::Facing;
    use petramond_world::tile::Tile;

    fn inst(facing: Facing, top: bool, open01: f32) -> TrapdoorInstance {
        TrapdoorInstance {
            pos: glam::IVec3::new(10, 64, -5),
            facing,
            top,
            open01,
            top_tile: Tile::named("oak_trapdoor"),
            bottom_tile: Tile::named("oak_trapdoor"),
            side_tile: Tile::named("oak_planks"),
            skylight: super::super::lighting::FULL_SKYLIGHT,
            blocklight: petramond_world::light::BlockLight6::DARK,
        }
    }

    fn bake(instances: &[TrapdoorInstance]) -> Vec<Vertex> {
        let (mut v, mut i) = (Vec::new(), Vec::new());
        push_trapdoors(instances, petramond_math::math::IVec3::ZERO, &mut v, &mut i);
        v
    }

    fn span(v: &[Vertex], axis: usize) -> (f32, f32) {
        v.iter().fold((f32::MAX, f32::MIN), |(lo, hi), vert| {
            (lo.min(vert.pos[axis]), hi.max(vert.pos[axis]))
        })
    }

    #[test]
    fn a_swung_panel_keeps_to_its_own_cell() {
        // The point of the inset hinge: the panel's RESTING poses lie exactly
        // in the cell, and mid-swing the corner nearest the hinge sweeps only
        // the tiny arc a rigid rotation about an inset pivot must — bounded by
        // (√2 - 1)·T/2, reached at 45°. A corner pivot would instead throw a
        // whole thickness of panel into the neighbouring cell at rest.
        let arc = (std::f32::consts::SQRT_2 - 1.0) * THICKNESS / 2.0 + 1e-4;
        for &facing in &[Facing::North, Facing::South, Facing::West, Facing::East] {
            for &top in &[false, true] {
                for step in 0..=4 {
                    let open01 = step as f32 / 4.0;
                    let slack = if step == 0 || step == 4 { 1e-4 } else { arc };
                    let v = bake(&[inst(facing, top, open01)]);
                    for (axis, origin) in [(0, 10.0), (1, 64.0), (2, -5.0)] {
                        let (lo, hi) = span(&v, axis);
                        assert!(
                            lo >= origin - slack && hi <= origin + 1.0 + slack,
                            "{facing:?} top={top} open={step}/4: axis {axis} {lo}..{hi} \
                             escaped the cell at {origin}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn opening_lifts_the_panel_off_the_floor_onto_an_edge() {
        let closed = bake(&[inst(Facing::South, false, 0.0)]);
        let open = bake(&[inst(Facing::South, false, 1.0)]);
        let thickness = |v: &[Vertex], axis: usize| {
            let (lo, hi) = span(v, axis);
            hi - lo
        };
        assert!(thickness(&closed, 1) < 0.25, "closed: thin on Y");
        assert!(thickness(&open, 1) > 0.9, "open: full height");
        assert!(thickness(&open, 2) < 0.25, "open: thin on Z (on its edge)");
    }
}
