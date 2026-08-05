//! World-query surface of a model block: per-cell collision boxes, selection/targeting
//! boxes, the break-overlay render boxes, the raycast outline, and the pixel-perfect
//! ray pick.

use glam::{Mat4, Vec3};

use crate::bbmodel::{euler_quat, face_corners};
use crate::block::Aabb;
use crate::facing::Facing;
use petramond_math::face::Face;

use super::atlas::{atlas, ModelAtlas};
use super::{instance, BlockModelKind, ModelCube};

/// The cell-local player-collision boxes for the cell at `offset` within the footprint.
/// `&'static` because the baked boxes live in the process-lifetime `INSTANCES`.
#[inline]
pub fn collision_boxes(kind: BlockModelKind, offset: [u8; 3]) -> &'static [Aabb] {
    match instance(kind).cell(offset) {
        Some(c) => &c.collision,
        None => &[],
    }
}

/// The cell-local player-collision boxes after applying a placement facing.
#[inline]
pub fn collision_boxes_oriented(
    kind: BlockModelKind,
    offset: [u8; 3],
    facing: Facing,
) -> &'static [Aabb] {
    match instance(kind).oriented_cell(offset, facing) {
        Some(c) => &c.collision,
        None => &[],
    }
}

/// The cell-local raycast TARGET box for the cell at `offset` (the geometry overlapping
/// it), or `None` if that cell has no targetable geometry. This is what the DDA tests; the
/// drawn outline is the whole-model box ([`outline_bounds`]).
#[inline]
pub fn selection_aabb(kind: BlockModelKind, offset: [u8; 3]) -> Option<([f32; 3], [f32; 3])> {
    let c = instance(kind).cell(offset)?;
    if c.selection_min == c.selection_max {
        return None;
    }
    Some((c.selection_min, c.selection_max))
}

/// The cell-local raycast target box after applying a placement facing.
#[inline]
pub fn selection_aabb_oriented(
    kind: BlockModelKind,
    offset: [u8; 3],
    facing: Facing,
) -> Option<([f32; 3], [f32; 3])> {
    let c = instance(kind).oriented_cell(offset, facing)?;
    if c.selection_min == c.selection_max {
        return None;
    }
    Some((c.selection_min, c.selection_max))
}

/// The FOOTPRINT-space posed cube boxes (the WHOLE model, one per cube) the break-crack
/// overlay paints over, so the crack hugs the model's real surfaces (every leg + the top)
/// rather than floating in the cell's air. The caller adds the footprint-origin world
/// cell. The whole multi-block breaks as one object, so the whole model cracks (MC-like).
#[inline]
pub fn model_render_boxes(kind: BlockModelKind) -> &'static [Aabb] {
    &instance(kind).cube_boxes
}

/// The whole model's tight bounding box in FOOTPRINT space (relative to the footprint
/// origin) — the black raycast outline, baked from geometry. The caller adds the world
/// origin so the wireframe hugs the model's real extent as ONE box across all its cells.
#[inline]
pub fn outline_bounds(kind: BlockModelKind) -> ([f32; 3], [f32; 3]) {
    let i = instance(kind);
    (i.bounds_min, i.bounds_max)
}

// ---------------------------------------------------------------------------------
// Pixel-perfect ray pick
// ---------------------------------------------------------------------------------

/// First-crossing distance of the ray through the model's SOLID, NON-TRANSPARENT
/// surface — every posed cube face is tested, and each candidate face is alpha-tested
/// against the model texture so a hit registers only on an opaque texel. Transparent
/// texels do NOT make the whole cube vanish from picking: the ray continues to later
/// faces, matching the renderer. The model pipelines cull BACK faces, so a face met
/// from behind (the far side of a cube, seen through the near face's cutout texels)
/// is skipped here exactly as it is not drawn. The ray is in
/// FOOTPRINT space (1 unit = 1 world cell; the caller subtracts the footprint-origin
/// world cell), matching `ModelInstance::cubes`. `None` on a clean miss — so aiming
/// through the gap between the legs, under the top, or through fully transparent model
/// texels does NOT select the block. Flat/degenerate decoration cubes (a plane, a
/// locator) are skipped.
pub fn ray_vs_model(eye: Vec3, dir: Vec3, kind: BlockModelKind) -> Option<f32> {
    let inst = instance(kind);
    let at = atlas();
    ray_vs_model_cubes(eye, dir, &inst.cubes, |cube, face, mn, mx, hit| {
        face_texel_opaque(cube, face, mn, mx, hit, at)
    })
}

/// [`ray_vs_model`] restricted to crossings inside the FOOTPRINT-space box
/// `[wmn, wmx]` — the per-cell form the DDA needs over an overhanging model
/// (see [`ray_vs_model_cubes_within`] for why a global first crossing is
/// wrong there).
pub fn ray_vs_model_within(
    eye: Vec3,
    dir: Vec3,
    kind: BlockModelKind,
    wmn: Vec3,
    wmx: Vec3,
) -> Option<f32> {
    let inst = instance(kind);
    let at = atlas();
    ray_vs_model_cubes_within(
        eye,
        dir,
        &inst.cubes,
        Some((wmn, wmx)),
        |cube, face, mn, mx, hit| face_texel_opaque(cube, face, mn, mx, hit, at),
    )
}

fn ray_vs_model_cubes<F>(
    eye: Vec3,
    dir: Vec3,
    cubes: &[ModelCube],
    face_opaque: F,
) -> Option<f32>
where
    F: FnMut(&ModelCube, Face, Vec3, Vec3, Vec3) -> bool,
{
    ray_vs_model_cubes_within(eye, dir, cubes, None, face_opaque)
}

/// [`ray_vs_model_cubes`] with an optional FOOTPRINT-space acceptance box: a
/// face crossing counts only when its surface point lies inside `within`.
/// This exists for per-cell attribution over a model whose geometry OVERHANGS
/// its footprint (`fit: native`): the DDA tests the whole model from each
/// footprint cell and must take the nearest crossing INSIDE that cell — with
/// only a global first-crossing, an out-of-footprint horn met first would
/// veto the in-cell body behind it, and the ray would select whatever block
/// is beyond the machine (the anvil, 2026-08-05). The overhang itself stays
/// deliberately unselectable (selection never extends beyond the footprint).
fn ray_vs_model_cubes_within<F>(
    eye: Vec3,
    dir: Vec3,
    cubes: &[ModelCube],
    within: Option<(Vec3, Vec3)>,
    mut face_opaque: F,
) -> Option<f32>
where
    F: FnMut(&ModelCube, Face, Vec3, Vec3, Vec3) -> bool,
{
    // The same tolerance the DDA's own cell attribution uses (`hit_in_cell`),
    // so a crossing exactly on a cell seam is accepted by both or neither.
    const SEAM: f32 = 1e-3;
    let mut best = f32::INFINITY;
    for cube in cubes {
        let mn = cube.from.min(cube.to);
        let mx = cube.from.max(cube.to);
        // Skip degenerate (flat plane / zero-extent locator) cubes — decoration, not a
        // pick target, and a zero-thickness slab can't be crossed cleanly anyway.
        if (mx - mn).min_element() <= 1e-4 {
            continue;
        }
        // Un-pose the ray into the cube's local axis-aligned frame (the static tilt is a
        // rigid rotate about the pivot, so distances along the ray are preserved).
        let tilt = Mat4::from_translation(cube.origin)
            * Mat4::from_quat(euler_quat(cube.rotation))
            * Mat4::from_translation(-cube.origin);
        let inv = tilt.inverse();
        let ol = inv.transform_point3(eye);
        let dl = inv.transform_vector3(dir);

        for face in Face::ALL {
            // Back-face rejection, mirroring the renderer's culling: a ray
            // travelling WITH the face's outward normal meets the face's back
            // side, which the model pass never draws (the far side of a cube
            // seen through the near face's cutout, or every face from inside
            // the cube) — so it must not pick either.
            let (nx, ny, nz) = face.dir();
            if dl.dot(Vec3::new(nx as f32, ny as f32, nz as f32)) > 0.0 {
                continue;
            }
            let Some((t, hit)) = ray_box_face_hit(ol, dl, mn, mx, face) else {
                continue;
            };
            if t >= best {
                continue;
            }
            if let Some((wmn, wmx)) = within {
                // The acceptance box is in the shared (posed) frame; the hit is
                // in the cube's local frame — re-pose it before comparing.
                let posed = tilt.transform_point3(hit);
                if posed.x < wmn.x - SEAM
                    || posed.y < wmn.y - SEAM
                    || posed.z < wmn.z - SEAM
                    || posed.x > wmx.x + SEAM
                    || posed.y > wmx.y + SEAM
                    || posed.z > wmx.z + SEAM
                {
                    continue;
                }
            }
            // Pixel-perfect: only an OPAQUE texel of this visible face counts. If the
            // nearer face is cut out here, a later face may still be the first rendered
            // pixel along the ray.
            if face_opaque(cube, face, mn, mx, hit) {
                best = t;
            }
        }
    }
    best.is_finite().then_some(best)
}

/// Ray vs one face of the local axis-aligned box `[mn, mx]`: the crossing distance plus
/// the local hit point. The hit test itself is side-agnostic — the CALLER decides
/// sidedness (the pixel-perfect pick rejects back-facing hits to mirror the model
/// pipelines' back-face culling; the AO bake tests occluders from both sides);
/// alpha still decides whether that face contributes a visible/pickable pixel.
pub(super) fn ray_box_face_hit(
    o: Vec3,
    d: Vec3,
    mn: Vec3,
    mx: Vec3,
    face: Face,
) -> Option<(f32, Vec3)> {
    let (axis, plane) = match face {
        Face::PosX => (0, mx.x),
        Face::NegX => (0, mn.x),
        Face::PosY => (1, mx.y),
        Face::NegY => (1, mn.y),
        Face::PosZ => (2, mx.z),
        Face::NegZ => (2, mn.z),
    };
    if d[axis].abs() < 1e-9 {
        return None;
    }
    let t = (plane - o[axis]) / d[axis];
    if t < -1e-6 {
        return None;
    }
    let t = t.max(0.0);
    let hit = o + d * t;
    for i in 0..3 {
        if i == axis {
            continue;
        }
        if hit[i] < mn[i] - 1e-5 || hit[i] > mx[i] + 1e-5 {
            return None;
        }
    }
    Some((t, hit))
}

/// Is the texel where the ray meets `cube`'s `face` opaque in the model texture? Solves
/// the local hit point against the face quad's two edge vectors for its `(s, t)`
/// fractions, maps those to the face's atlas-UV rect, and samples the atlas alpha. A
/// face the cube omits (no texture there) counts as opaque — the cube body is still
/// solid, that side is just an untextured interior seam.
pub(super) fn face_texel_opaque(
    cube: &ModelCube,
    face: Face,
    mn: Vec3,
    mx: Vec3,
    hit: Vec3,
    at: &ModelAtlas,
) -> bool {
    let slot = Face::ALL.iter().position(|&f| f == face).unwrap_or(0);
    let Some([u0, v0, u1, v1]) = cube.faces[slot] else {
        return true;
    };
    // face_corners order: bl, br, tr, tl. Edge vectors from bl span the face.
    let c = face_corners(face, mn, mx);
    let bl = Vec3::from(c[0]);
    let es = Vec3::from(c[1]) - bl; // bl -> br (horizontal)
    let et = Vec3::from(c[3]) - bl; // bl -> tl (vertical)
    let rel = hit - bl;
    let s = (rel.dot(es) / es.length_squared().max(1e-12)).clamp(0.0, 1.0);
    let t = (rel.dot(et) / et.length_squared().max(1e-12)).clamp(0.0, 1.0);
    // Corner UVs (mirroring `item_model::build_block_model_item`): bl=(u0,v1),
    // br=(u1,v1), tr=(u1,v0), tl=(u0,v0).
    let u = u0 + s * (u1 - u0);
    let v = v1 + t * (v0 - v1);
    at.alpha_at([u, v]) >= 128
}

#[cfg(test)]
mod tests {
    use super::*;

    const WB: BlockModelKind = BlockModelKind::FurnitureWorkbench;

    #[test]
    fn ray_pick_is_shape_aware_not_a_solid_box() {
        // Pixel-perfect pick: casting a grid of rays straight through the model's
        // footprint, SOME hit solid cubes and SOME pass through the gaps (between the
        // legs, under the top). A coarse per-cell box would make EVERY in-bounds ray
        // hit; the contrast (0 < hits < total) is what proves the pick follows the
        // actual geometry. Anchor-free: it pins no specific cube, only the shape-aware
        // behaviour.
        let (mn, mx) = outline_bounds(WB);
        let mut hits = 0;
        let mut total = 0;
        let n = 11;
        for i in 0..n {
            for j in 0..n {
                // Sample inside the XY bounds, cast front-to-back along +Z.
                let fx = (i as f32 + 0.5) / n as f32;
                let fy = (j as f32 + 0.5) / n as f32;
                let x = mn[0] + fx * (mx[0] - mn[0]);
                let y = mn[1] + fy * (mx[1] - mn[1]);
                let eye = Vec3::new(x, y, mn[2] - 0.5);
                total += 1;
                if ray_vs_model(eye, Vec3::Z, WB).is_some() {
                    hits += 1;
                }
            }
        }
        assert!(hits > 0, "the model must be pickable somewhere");
        assert!(
            hits < total,
            "some rays must pass through the model's gaps (not a solid box): {hits}/{total}"
        );
    }

    #[test]
    fn ray_pick_continues_past_transparent_near_face() {
        let cube = ModelCube {
            name: String::new(),
            from: Vec3::ZERO,
            to: Vec3::ONE,
            origin: Vec3::ZERO,
            rotation: Vec3::ZERO,
            faces: [Some([0.0, 0.0, 1.0, 1.0]); 6],
            cull: [None; 6],
        };

        // Through the transparent TOP face, the cube's own bottom face is met
        // from its BACK side — culled by the model pipelines, so not pickable
        // either: nothing renders along this ray, so nothing may pick.
        assert!(
            ray_vs_model_cubes(
                Vec3::new(0.5, 2.0, 0.5),
                Vec3::NEG_Y,
                std::slice::from_ref(&cube),
                |_, face, _, _, _| face == Face::NegY,
            )
            .is_none(),
            "the far face's back side is neither drawn nor pickable"
        );

        // A SECOND cube behind the first picks on its front side — the ray
        // genuinely continues past the cut-out near face.
        let below = ModelCube {
            from: Vec3::new(0.0, -1.0, 0.0),
            to: Vec3::new(1.0, 0.0, 1.0),
            ..cube.clone()
        };
        let hit = ray_vs_model_cubes(
            Vec3::new(0.5, 2.0, 0.5),
            Vec3::NEG_Y,
            &[cube.clone(), below],
            |_, face, _, mx, _| mx.y <= 0.0 && face == Face::PosY,
        )
        .expect("the lower cube's top face is front-facing and opaque");
        assert!((hit - 2.0).abs() < 1e-5, "ray should hit at y=0, got {hit}");
    }

    /// The per-cell acceptance box: an overhanging horn met FIRST must not
    /// veto the in-cell body behind it — the anvil bug (2026-08-05), where a
    /// ray clipping the out-of-footprint horn selected the wall beyond the
    /// machine. The overhang itself stays unselectable inside the box.
    #[test]
    fn a_crossing_outside_the_acceptance_box_yields_to_one_inside() {
        let cube = |from: Vec3, to: Vec3| ModelCube {
            name: String::new(),
            from,
            to,
            origin: Vec3::ZERO,
            rotation: Vec3::ZERO,
            faces: [Some([0.0, 0.0, 1.0, 1.0]); 6],
            cull: [None; 6],
        };
        // A horn overhanging past x=1 and the body inside 0..1.
        let horn = cube(Vec3::new(1.0, 0.4, 0.4), Vec3::new(1.3, 0.8, 0.8));
        let body = cube(Vec3::new(0.1, 0.0, 0.1), Vec3::new(0.9, 0.75, 0.9));
        let cubes = [horn, body];
        // A ray from +X that clips the horn first, then reaches the body.
        let eye = Vec3::new(3.0, 0.6, 0.6);
        let dir = Vec3::NEG_X;
        let all = |_: &ModelCube, _: Face, _: Vec3, _: Vec3, _: Vec3| true;
        // Unrestricted: the horn is the first crossing.
        let first = ray_vs_model_cubes(eye, dir, &cubes, all).expect("hits the horn");
        assert!((first - 1.7).abs() < 1e-4, "horn face at x=1.3, got {first}");
        // Restricted to the 0..1 cell: the horn is skipped, the body picks.
        let within = ray_vs_model_cubes_within(
            eye,
            dir,
            &cubes,
            Some((Vec3::ZERO, Vec3::ONE)),
            all,
        )
        .expect("the in-cell body still picks");
        assert!((within - 2.1).abs() < 1e-4, "body face at x=0.9, got {within}");
        // And a box that covers neither crossing yields a clean miss.
        assert!(ray_vs_model_cubes_within(
            eye,
            dir,
            &cubes,
            Some((Vec3::new(2.0, 0.0, 0.0), Vec3::new(3.0, 1.0, 1.0))),
            all,
        )
        .is_none());
    }

    #[test]
    fn ray_pick_takes_a_solid_faces_front_side() {
        // The same face from OUTSIDE is front-facing and picks normally; the
        // back-face rejection must not eat ordinary picks.
        let cube = ModelCube {
            name: String::new(),
            from: Vec3::ZERO,
            to: Vec3::ONE,
            origin: Vec3::ZERO,
            rotation: Vec3::ZERO,
            faces: [Some([0.0, 0.0, 1.0, 1.0]); 6],
            cull: [None; 6],
        };
        let hit = ray_vs_model_cubes(
            Vec3::new(0.5, -2.0, 0.5),
            Vec3::Y,
            std::slice::from_ref(&cube),
            |_, face, _, _, _| face == Face::NegY,
        )
        .expect("the bottom face from below is front-facing");
        assert!((hit - 2.0).abs() < 1e-5, "ray should hit at y=0, got {hit}");
    }

    #[test]
    fn ray_far_outside_the_model_misses() {
        // A ray nowhere near the footprint never registers a hit.
        assert!(ray_vs_model(Vec3::new(100.0, 100.0, 100.0), Vec3::Z, WB).is_none());
    }
}
