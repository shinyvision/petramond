//! World-query surface of a model block: per-cell collision boxes, selection/targeting
//! boxes, the break-overlay render boxes, the raycast outline, and the pixel-perfect
//! ray pick.

use glam::{Mat4, Vec3};

use crate::bbmodel::{euler_quat, face_corners};
use crate::block::Aabb;
use crate::facing::Facing;
use crate::mesh::face::Face;

use super::atlas::{atlas, ModelAtlas};
use super::{instance, BlockModelKind, ModelCube};

/// The cell-local player-collision boxes for the cell at `offset` within the footprint.
/// `&'static` because the baked boxes live in the process-lifetime [`INSTANCES`].
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
/// faces, matching the renderer's double-sided alpha-cutout model pass. The ray is in
/// FOOTPRINT space (1 unit = 1 world cell; the caller subtracts the footprint-origin
/// world cell), matching [`ModelInstance::cubes`]. `None` on a clean miss — so aiming
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

fn ray_vs_model_cubes<F>(
    eye: Vec3,
    dir: Vec3,
    cubes: &[ModelCube],
    mut face_opaque: F,
) -> Option<f32>
where
    F: FnMut(&ModelCube, Face, Vec3, Vec3, Vec3) -> bool,
{
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
            let Some((t, hit)) = ray_box_face_hit(ol, dl, mn, mx, face) else {
                continue;
            };
            if t >= best {
                continue;
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
/// the local hit point. Faces are treated as double-sided because the model pass disables
/// culling; alpha still decides whether that face contributes a visible/pickable pixel.
pub(super) fn ray_box_face_hit(o: Vec3, d: Vec3, mn: Vec3, mx: Vec3, face: Face) -> Option<(f32, Vec3)> {
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
        };

        let hit = ray_vs_model_cubes(
            Vec3::new(0.5, 2.0, 0.5),
            Vec3::NEG_Y,
            std::slice::from_ref(&cube),
            |_, face, _, _, _| face == Face::NegY,
        )
        .expect("bottom face should be pickable through transparent top");

        assert!(
            (hit - 2.0).abs() < 1e-5,
            "ray should hit the later opaque face, got {hit}"
        );
    }

    #[test]
    fn ray_far_outside_the_model_misses() {
        // A ray nowhere near the footprint never registers a hit.
        assert!(ray_vs_model(Vec3::new(100.0, 100.0, 100.0), Vec3::Z, WB).is_none());
    }
}
