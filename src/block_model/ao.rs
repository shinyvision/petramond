//! Startup-baked ambient occlusion for bbmodel blocks.
//!
//! Two bakes, both computed once per model kind in [`ModelInstance::build`]
//! (footprint space, after the fit — so every distance below is in WORLD pixels,
//! 1/16 of a cell, regardless of the authored scale or fit mode):
//!
//! - **Face AO** ([`bake_face_ao`]): a per-face, per-corner shade multiplier from
//!   deterministic hemisphere rays against the model's OTHER cuboids (posed OBBs).
//!   Short reach and a hard darkening cap make it joint definition (a leg meeting
//!   the tabletop, slats meeting a frame), never a soot pass. The factors multiply
//!   into the template vertex `shade`, so chunk meshes, held/dropped items, and
//!   inventory icons all shade identically with zero per-remesh cost.
//! - **Contact field** ([`bake_contact_field`]): a per-bottom-cell scalar darkening
//!   field from cuboids resting near the model floor — the soft stamp the mesher
//!   lays on opaque terrain directly under the model (see
//!   `mesh::builder::model_block`).
//!
//! Small-part guards, shared by both bakes: an element at or below
//! [`THIN_MIN`] minimum thickness casts NOTHING (planes and decals receive AO but
//! never cast it), participation fades in through [`THIN_FULL`], and casters
//! MAX-combine (per ray / per texel) so clustered detail cannot stack into a black
//! knot. Rays are alpha-aware: a hit samples the face's atlas texel, so a cutout
//! texel lets the ray continue instead of casting a solid patch.

use glam::{Mat4, Vec3};

use crate::bbmodel::{euler_quat, face_corners};
use crate::mesh::face::Face;

use super::geometry::posed_cube_bounds;
use super::query::ray_box_face_hit;
use super::ModelCube;

/// One WORLD pixel in footprint space (16 px = 1 cell).
const PX: f32 = 1.0 / 16.0;
/// How far a caster can reach: occlusion falls linearly to zero at this distance.
const REACH: f32 = 2.0 * PX;
/// Maximum total darkening of a fully occluded corner (shade multiplier `1 - CAP`).
const MAX_DARKEN: f32 = 0.5;
/// Minimum caster thickness: at or below this an element casts nothing.
const THIN_MIN: f32 = 0.5 * PX;
/// Full caster participation at or above this thickness.
const THIN_FULL: f32 = 2.0 * PX;
/// Ray origins are lifted off the face plane so a ray never re-hits geometry at
/// or below its own surface (flush coplanar neighbours forming one continuous
/// surface are geometrically unreachable by a rising ray).
const ORIGIN_LIFT: f32 = 0.25 * PX;
/// A hit must rise at least this far above the receiving face plane to count —
/// the numerical backstop behind the lifted-origin guarantee.
const MIN_RISE: f32 = 0.1 * PX;

/// Contact stamp: maximum darkening of the terrain texel under the model.
const CONTACT_MAX_DARKEN: f32 = 0.3;
/// Vertical reach of the contact field: a cuboid floating higher than this above
/// the model floor stamps nothing (a tabletop casts no floor blob — its legs do).
const CONTACT_REACH: f32 = 2.0 * PX;
/// Horizontal falloff of the stamp beyond the cuboid's footprint edge.
const CONTACT_SPREAD: f32 = 10.0 * PX;
/// Corner-grid resolution of the per-cell contact field (an 8×8 quad lattice).
pub(super) const CONTACT_GRID: usize = 9;

/// Caster participation weight from the cuboid's minimum authored extent:
/// 0 at ≤ [`THIN_MIN`], fading to 1 at ≥ [`THIN_FULL`].
fn caster_weight(cube: &ModelCube) -> f32 {
    let thick = (cube.to - cube.from).abs().min_element();
    ((thick - THIN_MIN) / (THIN_FULL - THIN_MIN)).clamp(0.0, 1.0)
}

/// The cube's static tilt about its pivot (the same pose every other consumer
/// composes).
fn cube_tilt(cube: &ModelCube) -> Mat4 {
    Mat4::from_translation(cube.origin)
        * Mat4::from_quat(euler_quat(cube.rotation))
        * Mat4::from_translation(-cube.origin)
}

/// Deterministic hemisphere ray set in the face's tangent frame:
/// `(tangent, bitangent, normal)` coefficients. Two elevation rings of eight
/// azimuths plus the face normal — enough directions for a smooth gradient at
/// this reach, few enough that the whole bake is startup noise.
fn hemisphere_rays() -> Vec<[f32; 3]> {
    let mut rays = Vec::with_capacity(17);
    for &elev_deg in &[30.0f32, 60.0] {
        let (sin_e, cos_e) = elev_deg.to_radians().sin_cos();
        for k in 0..8 {
            let (sin_a, cos_a) = (k as f32 * 45.0f32).to_radians().sin_cos();
            rays.push([cos_e * cos_a, cos_e * sin_a, sin_e]);
        }
    }
    rays.push([0.0, 0.0, 1.0]);
    rays
}

/// Per-caster precomputation for the ray casts.
struct Caster {
    tilt: Mat4,
    inv_tilt: Mat4,
    mn: Vec3,
    mx: Vec3,
    weight: f32,
}

/// Bake the per-cube, per-face (`Face::ALL` slot order), per-corner
/// (`face_corners` order) shade multipliers. `face_opaque(cube, face, mn, mx,
/// local_hit)` answers whether the caster's texel at the hit is opaque — the
/// production closure samples the model atlas exactly like the pixel-perfect ray
/// pick; tests inject constants. Faces a cube omits keep 1.0.
pub(super) fn bake_face_ao(
    cubes: &[ModelCube],
    face_opaque: impl Fn(&ModelCube, Face, Vec3, Vec3, Vec3) -> bool,
) -> Vec<[[f32; 4]; 6]> {
    let rays = hemisphere_rays();
    let casters: Vec<Caster> = cubes
        .iter()
        .map(|c| {
            let tilt = cube_tilt(c);
            Caster {
                tilt,
                inv_tilt: tilt.inverse(),
                mn: c.from.min(c.to),
                mx: c.from.max(c.to),
                weight: caster_weight(c),
            }
        })
        .collect();

    cubes
        .iter()
        .enumerate()
        .map(|(ri, cube)| {
            let mut per_face = [[1.0f32; 4]; 6];
            let tilt = &casters[ri].tilt;
            for (slot, face) in Face::ALL.into_iter().enumerate() {
                if cube.faces[slot].is_none() {
                    continue;
                }
                let local = face_corners(face, cube.from, cube.to);
                let es = Vec3::from(local[1]) - Vec3::from(local[0]);
                let et = Vec3::from(local[3]) - Vec3::from(local[0]);
                // Degenerate tangent frame (a cube flat on two+ axes) — never
                // emitted anyway.
                if es.length_squared() < 1e-10 || et.length_squared() < 1e-10 {
                    continue;
                }
                let t_axis = tilt.transform_vector3(es).normalize();
                let normal = tilt.transform_vector3(es.cross(et).normalize()).normalize();
                let b_axis = normal.cross(t_axis);
                for (ci, corner) in local.into_iter().enumerate() {
                    let posed = tilt.transform_point3(Vec3::from(corner));
                    let origin = posed + normal * ORIGIN_LIFT;
                    let mut occ_sum = 0.0f32;
                    for ray in &rays {
                        let dir = t_axis * ray[0] + b_axis * ray[1] + normal * ray[2];
                        let mut best = 0.0f32;
                        for (oi, other) in casters.iter().enumerate() {
                            if oi == ri || other.weight <= 0.0 {
                                continue;
                            }
                            let ol = other.inv_tilt.transform_point3(origin);
                            let dl = other.inv_tilt.transform_vector3(dir);
                            for hit_face in Face::ALL {
                                let Some((t, hit)) =
                                    ray_box_face_hit(ol, dl, other.mn, other.mx, hit_face)
                                else {
                                    continue;
                                };
                                if t > REACH {
                                    continue;
                                }
                                let contrib = other.weight * (1.0 - t / REACH);
                                if contrib <= best {
                                    continue;
                                }
                                // Only geometry genuinely RISING above the
                                // receiving plane occludes; a flush coplanar
                                // continuation of the same surface does not.
                                let hit_fp = other.tilt.transform_point3(hit);
                                if (hit_fp - posed).dot(normal) < MIN_RISE {
                                    continue;
                                }
                                if !face_opaque(&cubes[oi], hit_face, other.mn, other.mx, hit) {
                                    continue;
                                }
                                best = contrib;
                            }
                        }
                        occ_sum += best;
                    }
                    per_face[slot][ci] = 1.0 - MAX_DARKEN * (occ_sum / rays.len() as f32);
                }
            }
            per_face
        })
        .collect()
}

/// Bake the contact-shadow field for the floor cell at `(cx, cz)` — a bottom
/// footprint cell OR a ring cell of the one-cell dilation around the footprint
/// (coordinates may be `-1` / `footprint`): darkening at the corners of an 8×8
/// lattice over that cell's floor, from substantial cuboids within
/// [`CONTACT_REACH`] of the model floor (`y = 0` in footprint space). The
/// one-cell ring is sufficient by construction: [`CONTACT_SPREAD`] plus any
/// authored caster overhang stays within 16 px of the footprint edge. Per-caster
/// contributions MAX-combine; `None` when nothing near this cell reaches the
/// floor.
pub(super) fn bake_contact_field(
    cubes: &[ModelCube],
    cx: i32,
    cz: i32,
) -> Option<[[f32; CONTACT_GRID]; CONTACT_GRID]> {
    struct FloorCaster {
        mn: Vec3,
        mx: Vec3,
        strength: f32,
    }
    let floor_casters: Vec<FloorCaster> = cubes
        .iter()
        .filter_map(|c| {
            let weight = caster_weight(c);
            if weight <= 0.0 {
                return None;
            }
            let (mn, mx) = posed_cube_bounds(c);
            let lift = mn.y.max(0.0);
            if lift > CONTACT_REACH {
                return None;
            }
            let height_fade = 1.0 - lift / CONTACT_REACH;
            Some(FloorCaster {
                mn,
                mx,
                strength: weight * height_fade,
            })
        })
        .collect();
    if floor_casters.is_empty() {
        return None;
    }

    let mut field = [[0.0f32; CONTACT_GRID]; CONTACT_GRID];
    let mut any = false;
    let step = 1.0 / (CONTACT_GRID - 1) as f32;
    for (i, row) in field.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            let px = cx as f32 + i as f32 * step;
            let pz = cz as f32 + j as f32 * step;
            let mut combined = 0.0f32;
            for c in &floor_casters {
                let dx = (c.mn.x - px).max(px - c.mx.x).max(0.0);
                let dz = (c.mn.z - pz).max(pz - c.mx.z).max(0.0);
                let dist = (dx * dx + dz * dz).sqrt();
                let falloff = 1.0 - (dist / CONTACT_SPREAD).min(1.0);
                combined = combined.max(c.strength * falloff);
            }
            *v = CONTACT_MAX_DARKEN * combined;
            any |= *v > 1e-4;
        }
    }
    any.then_some(field)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube(from: [f32; 3], to: [f32; 3]) -> ModelCube {
        ModelCube {
            name: String::new(),
            from: Vec3::from(from),
            to: Vec3::from(to),
            origin: Vec3::ZERO,
            rotation: Vec3::ZERO,
            faces: [Some([0.0, 0.0, 1.0, 1.0]); 6],
        }
    }

    fn slot(face: Face) -> usize {
        Face::ALL.iter().position(|&f| f == face).unwrap()
    }

    /// A table leg under a top: the leg's side-face corners TOUCHING the top
    /// darken, the corners at the floor stay unshaded, and the cap holds.
    #[test]
    fn joint_corners_darken_within_the_cap() {
        let cubes = vec![
            // Leg: 2px square column up to y=0.75.
            cube([0.4375, 0.0, 0.4375], [0.5625, 0.75, 0.5625]),
            // Top: full-cell slab above it.
            cube([0.0, 0.75, 0.0], [1.0, 0.875, 1.0]),
        ];
        let ao = bake_face_ao(&cubes, |_, _, _, _, _| true);
        let side = &ao[0][slot(Face::PosX)];
        // face_corners order is bl, br, tr, tl: the two `t` corners touch the top.
        assert!(
            side[2] < 1.0 && side[3] < 1.0,
            "top corners darken: {side:?}"
        );
        assert!(
            side[0] > side[2] && side[1] > side[3],
            "floor corners stay brighter: {side:?}"
        );
        for f in &ao {
            for corners in f {
                for &v in corners {
                    assert!(
                        (1.0 - MAX_DARKEN - 1e-4..=1.0 + 1e-6).contains(&v),
                        "cap violated: {v}"
                    );
                }
            }
        }
    }

    /// Thin elements (≤ 0.5 px) cast nothing — a decal plane next to a face
    /// leaves it fully lit.
    #[test]
    fn thin_casters_cast_nothing() {
        let cubes = vec![
            cube([0.0, 0.0, 0.0], [0.5, 0.5, 0.5]),
            // A plane rising flush against the first cube's +X face.
            cube([0.51, 0.0, 0.0], [0.51, 1.0, 0.5]),
        ];
        let ao = bake_face_ao(&cubes, |_, _, _, _, _| true);
        for corners in &ao[0] {
            for &v in corners {
                assert!((v - 1.0).abs() < 1e-6, "thin caster must not darken: {v}");
            }
        }
    }

    /// Two flush cubes forming one continuous surface: no darkening anywhere on
    /// the shared top plane (the classic bake false-positive).
    #[test]
    fn flush_coplanar_surfaces_stay_unshaded() {
        let cubes = vec![
            cube([0.0, 0.0, 0.0], [0.5, 0.5, 1.0]),
            cube([0.5, 0.0, 0.0], [1.0, 0.5, 1.0]),
        ];
        let ao = bake_face_ao(&cubes, |_, _, _, _, _| true);
        for c in 0..2 {
            let top = &ao[c][slot(Face::PosY)];
            for &v in top {
                assert!((v - 1.0).abs() < 1e-6, "flush seam must stay unshaded: {v}");
            }
        }
    }

    /// A caster whose texels are all transparent occludes nothing.
    #[test]
    fn transparent_casters_cast_nothing() {
        let cubes = vec![
            cube([0.4, 0.0, 0.4], [0.6, 0.5, 0.6]),
            cube([0.0, 0.5, 0.0], [1.0, 0.75, 1.0]),
        ];
        let opaque = bake_face_ao(&cubes, |_, _, _, _, _| true);
        let transparent = bake_face_ao(&cubes, |_, _, _, _, _| false);
        let side = slot(Face::PosX);
        assert!(
            opaque[0][side].iter().any(|&v| v < 1.0),
            "sanity: the opaque bake darkens the joint"
        );
        for corners in &transparent[0] {
            for &v in corners {
                assert!(
                    (v - 1.0).abs() < 1e-6,
                    "transparent texels must not cast: {v}"
                );
            }
        }
    }

    /// The contact field darkens under near-floor geometry, fades out with
    /// distance, ignores thin planes and floating cuboids, and holds its cap.
    #[test]
    fn contact_field_covers_floor_geometry_only() {
        // A leg in the cell's -X/-Z quarter.
        let leg = cube([0.1, 0.0, 0.1], [0.3, 0.8, 0.3]);
        let field = bake_contact_field(&[leg.clone()], 0, 0).expect("leg stamps");
        assert!(field[1][1] > 0.0, "under the leg darkens");
        assert!(
            field[CONTACT_GRID - 1][CONTACT_GRID - 1] == 0.0,
            "the far corner is out of reach"
        );
        assert!(field[1][1] <= CONTACT_MAX_DARKEN + 1e-6, "cap holds");

        let plane = cube([0.1, 0.0, 0.1], [0.3, 0.0, 0.3]);
        assert!(
            bake_contact_field(&[plane], 0, 0).is_none(),
            "a thin plane stamps nothing"
        );

        let floating = cube([0.1, 0.5, 0.1], [0.3, 0.8, 0.3]);
        assert!(
            bake_contact_field(&[floating], 0, 0).is_none(),
            "geometry above the reach stamps nothing"
        );
    }
}
