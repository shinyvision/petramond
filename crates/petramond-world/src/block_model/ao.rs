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
use petramond_math::face::Face;

use super::geometry::posed_cube_bounds;
use super::query::ray_box_face_hit;
use super::ModelCube;

const PX: f32 = 1.0 / 16.0;
const REACH: f32 = 2.0 * PX;
const MAX_DARKEN: f32 = 0.5;
const THIN_MIN: f32 = 0.5 * PX;
const THIN_FULL: f32 = 2.0 * PX;
const ORIGIN_LIFT: f32 = 0.25 * PX;
const MIN_RISE: f32 = 0.1 * PX;

const CONTACT_MAX_DARKEN: f32 = 0.3;
const CONTACT_REACH: f32 = 2.0 * PX;
const CONTACT_SPREAD: f32 = 10.0 * PX;
pub(super) const CONTACT_GRID: usize = 9;

fn caster_weight(cube: &ModelCube) -> f32 {
    let thick = (cube.to - cube.from).abs().min_element();
    ((thick - THIN_MIN) / (THIN_FULL - THIN_MIN)).clamp(0.0, 1.0)
}

fn cube_tilt(cube: &ModelCube) -> Mat4 {
    Mat4::from_translation(cube.origin)
        * Mat4::from_quat(euler_quat(cube.rotation))
        * Mat4::from_translation(-cube.origin)
}

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

pub struct AoBox {
    pub from: Vec3,
    pub to: Vec3,
    pub pose: Mat4,
    pub faces: [bool; 6],
    pub casts: bool,
}

struct Caster {
    pose: Mat4,
    inv_pose: Mat4,
    mn: Vec3,
    mx: Vec3,
    weight: f32,
    posed_mn: Vec3,
    posed_mx: Vec3,
}

fn posed_bounds(pose: &Mat4, mn: Vec3, mx: Vec3) -> (Vec3, Vec3) {
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    for i in 0..8 {
        let corner = Vec3::new(
            if i & 1 == 0 { mn.x } else { mx.x },
            if i & 2 == 0 { mn.y } else { mx.y },
            if i & 4 == 0 { mn.z } else { mx.z },
        );
        let p = pose.transform_point3(corner);
        lo = lo.min(p);
        hi = hi.max(p);
    }
    (lo, hi)
}

fn box_gap(amn: Vec3, amx: Vec3, bmn: Vec3, bmx: Vec3) -> f32 {
    (bmn - amx).max(amn - bmx).max(Vec3::ZERO).length()
}

/// True when no face test of `ray_box_face_hit` can report a hit with `t <= reach`: on some
/// axis the ray's coordinate over `t` in `[-1e-6, reach]` stays clear of the box's slab by more
/// than the face test's tolerance. Deliberately loose so it can only skip certain misses.
#[inline]
fn segment_misses_box(o: Vec3, d: Vec3, reach: f32, mn: Vec3, mx: Vec3) -> bool {
    const TOL: f32 = 1e-4;
    let a = o - d * 1e-6;
    let b = o + d * reach;
    let lo = a.min(b) - Vec3::splat(TOL);
    let hi = a.max(b) + Vec3::splat(TOL);
    hi.cmplt(mn).any() || lo.cmpgt(mx).any()
}

pub(super) fn bake_face_ao(
    cubes: &[ModelCube],
    face_opaque: impl Fn(&ModelCube, Face, Vec3, Vec3, Vec3) -> bool,
) -> Vec<[[f32; 4]; 6]> {
    let boxes: Vec<AoBox> = cubes
        .iter()
        .map(|c| AoBox {
            from: c.from,
            to: c.to,
            pose: cube_tilt(c),
            faces: c.faces.map(|f| f.is_some()),
            casts: true,
        })
        .collect();
    bake_box_ao(&boxes, PX, 1.0, |i, face, mn, mx, hit| {
        face_opaque(&cubes[i], face, mn, mx, hit)
    })
}

pub fn bake_box_ao(
    boxes: &[AoBox],
    px: f32,
    curve: f32,
    face_opaque: impl Fn(usize, Face, Vec3, Vec3, Vec3) -> bool,
) -> Vec<[[f32; 4]; 6]> {
    let unit = px / PX;
    let (reach, lift, min_rise) = (REACH * unit, ORIGIN_LIFT * unit, MIN_RISE * unit);
    let rays = hemisphere_rays();
    let casters: Vec<Caster> = boxes
        .iter()
        .map(|b| {
            let thick = (b.to - b.from).abs().min_element() / unit;
            let weight = ((thick - THIN_MIN) / (THIN_FULL - THIN_MIN)).clamp(0.0, 1.0);
            let (mn, mx) = (b.from.min(b.to), b.from.max(b.to));
            let (posed_mn, posed_mx) = posed_bounds(&b.pose, mn, mx);
            Caster {
                pose: b.pose,
                inv_pose: b.pose.inverse(),
                mn,
                mx,
                weight: if b.casts { weight } else { 0.0 },
                posed_mn,
                posed_mx,
            }
        })
        .collect();

    // A counted hit is the posed point `origin + dir * t` with `t <= reach`, and
    // it lies on the caster's posed box, so a caster whose posed bounds sit
    // farther than `reach * |dir|` from the ray origin cannot contribute.
    // Skipping it is exact: the per-ray combine is a max. `|dir| <= sqrt(3)`
    // bounds any frame; each face then uses its own rays' longest `dir`.
    let slack = 1e-3 * unit;
    let box_reach = lift + reach * 3f32.sqrt() + slack;

    boxes
        .iter()
        .enumerate()
        .map(|(ri, cube)| {
            let mut per_face = [[1.0f32; 4]; 6];
            let receiver = &casters[ri];
            let tilt = &receiver.pose;
            let nearby: Vec<usize> = casters
                .iter()
                .enumerate()
                .filter(|&(oi, other)| {
                    oi != ri
                        && other.weight > 0.0
                        && box_gap(
                            receiver.posed_mn,
                            receiver.posed_mx,
                            other.posed_mn,
                            other.posed_mx,
                        ) <= box_reach
                })
                .map(|(oi, _)| oi)
                .collect();
            let mut candidates: Vec<(usize, Vec3)> = Vec::with_capacity(nearby.len());
            for (slot, face) in Face::ALL.into_iter().enumerate() {
                if !cube.faces[slot] {
                    continue;
                }
                let local = face_corners(face, cube.from, cube.to);
                let es = Vec3::from(local[1]) - Vec3::from(local[0]);
                let et = Vec3::from(local[3]) - Vec3::from(local[0]);
                if es.length_squared() < 1e-10 || et.length_squared() < 1e-10 {
                    continue;
                }
                let t_axis = tilt.transform_vector3(es).normalize();
                let normal = tilt.transform_vector3(es.cross(et).normalize()).normalize();
                let b_axis = normal.cross(t_axis);
                let dirs: Vec<Vec3> = rays
                    .iter()
                    .map(|ray| t_axis * ray[0] + b_axis * ray[1] + normal * ray[2])
                    .collect();
                let dir_reach = reach * dirs.iter().fold(0.0f32, |m, d| m.max(d.length())) + slack;
                for (ci, corner) in local.into_iter().enumerate() {
                    let posed = tilt.transform_point3(Vec3::from(corner));
                    let origin = posed + normal * lift;
                    candidates.clear();
                    candidates.extend(nearby.iter().filter_map(|&oi| {
                        let other = &casters[oi];
                        (box_gap(origin, origin, other.posed_mn, other.posed_mx) <= dir_reach)
                            .then(|| (oi, other.inv_pose.transform_point3(origin)))
                    }));
                    let mut occ_sum = 0.0f32;
                    for &dir in &dirs {
                        let mut best = 0.0f32;
                        for &(oi, ol) in &candidates {
                            let other = &casters[oi];
                            let dl = other.inv_pose.transform_vector3(dir);
                            if segment_misses_box(ol, dl, reach, other.mn, other.mx) {
                                continue;
                            }
                            for hit_face in Face::ALL {
                                let Some((t, hit)) =
                                    ray_box_face_hit(ol, dl, other.mn, other.mx, hit_face)
                                else {
                                    continue;
                                };
                                if t > reach {
                                    continue;
                                }
                                let contrib = other.weight * (1.0 - t / reach);
                                if contrib <= best {
                                    continue;
                                }
                                let hit_fp = other.pose.transform_point3(hit);
                                if (hit_fp - posed).dot(normal) < min_rise {
                                    continue;
                                }
                                if !face_opaque(oi, hit_face, other.mn, other.mx, hit) {
                                    continue;
                                }
                                best = contrib;
                            }
                        }
                        occ_sum += best;
                    }
                    let occlusion = (occ_sum / rays.len() as f32).clamp(0.0, 1.0);
                    per_face[slot][ci] = 1.0 - MAX_DARKEN * occlusion.powf(curve);
                }
            }
            per_face
        })
        .collect()
}

/// Contact shadow for floor cell `(cx, cz)`. The cell can be in the footprint or in the ring one
/// cell outside it, so the coords may be `-1` or `footprint`. Casters are cuboids within
/// [`CONTACT_REACH`] of the floor. One ring is enough since [`CONTACT_SPREAD`] plus overhang stays
/// within 16px. Strongest caster wins. `None` if nothing reaches the floor.
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
            faces: [Some(crate::bbmodel::FaceUv::new([0.0, 0.0, 1.0, 1.0])); 6],
            cull: [None; 6],
        }
    }

    fn slot(face: Face) -> usize {
        Face::ALL.iter().position(|&f| f == face).unwrap()
    }

    #[test]
    fn joint_corners_darken_within_the_cap() {
        let cubes = vec![
            cube([0.4375, 0.0, 0.4375], [0.5625, 0.75, 0.5625]),
            cube([0.0, 0.75, 0.0], [1.0, 0.875, 1.0]),
        ];
        let ao = bake_face_ao(&cubes, |_, _, _, _, _| true);
        let side = &ao[0][slot(Face::PosX)];
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

    #[test]
    fn thin_casters_cast_nothing() {
        let cubes = vec![
            cube([0.0, 0.0, 0.0], [0.5, 0.5, 0.5]),
            cube([0.51, 0.0, 0.0], [0.51, 1.0, 0.5]),
        ];
        let ao = bake_face_ao(&cubes, |_, _, _, _, _| true);
        for corners in &ao[0] {
            for &v in corners {
                assert!((v - 1.0).abs() < 1e-6, "thin caster must not darken: {v}");
            }
        }
    }

    #[test]
    fn flush_coplanar_surfaces_stay_unshaded() {
        let cubes = vec![
            cube([0.0, 0.0, 0.0], [0.5, 0.5, 1.0]),
            cube([0.5, 0.0, 0.0], [1.0, 0.5, 1.0]),
        ];
        let ao = bake_face_ao(&cubes, |_, _, _, _, _| true);
        for faces in ao.iter().take(2) {
            let top = &faces[slot(Face::PosY)];
            for &v in top {
                assert!((v - 1.0).abs() < 1e-6, "flush seam must stay unshaded: {v}");
            }
        }
    }

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

    #[test]
    fn contact_field_covers_floor_geometry_only() {
        let leg = cube([0.1, 0.0, 0.1], [0.3, 0.8, 0.3]);
        let field = bake_contact_field(std::slice::from_ref(&leg), 0, 0).expect("leg stamps");
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
