//! World-space geometry for animated entity models (mobs), baked each frame into
//! the explicit-UV [`ItemVertex`](super::item_model::ItemVertex) stream and drawn by
//! the dedicated `mob` pipeline (see `pipeline.rs` / `mob.wgsl`).
//!
//! Generic over species: the caller passes the parsed [`Model`], its render `scale`,
//! and (optionally) the walk [`Animation`]; each instance is posed by its own
//! `anim_time` when `moving`, or in the model's neutral [rest pose](Model::rest_pose)
//! when idle (so a standing mob shows straight legs with no per-animation tuning).
//!
//! Like [`item_entity`](super::item_entity) / [`chest_model`](super::chest_model)
//! this bakes in WORLD space on the CPU (the mob pipeline's vertex shader applies
//! only `view_proj`). Per cube the transform is `G · pose[bone] · S_cube`, where
//! `S_cube` is the cube's modelled static tilt, `pose[bone]` the animation (or rest)
//! transform, and `G = T(pos)·yaw·scale` places the model in the world. Faces use the
//! same `quad_box` winding the chunk mesher + block models use, so the bbmodel's
//! per-face sub-rect UVs map upright. Per-face directional shade × the instance
//! skylight is folded into the vertex `shade`, matching `item_model`.

use glam::{Mat4, Vec3};

use super::item_model::ItemVertex;
use super::lighting::{light_rgb, DynLight, LightEnv};
use super::MobRenderInstance;
use crate::bbmodel::{euler_quat, face_corners, Animation, Model};
use crate::mesh::face::Face;
use crate::mesh::SHADES;

/// White: mobs are textured directly (no foliage tint), so the shader's
/// `tex.rgb * shade * tint` reduces to `tex.rgb * shade`.
const NO_TINT: [f32; 3] = [1.0, 1.0, 1.0];
/// The authored name of a model's shearable-coat cubes (a sheep's fleece): every cube
/// with this element name is skipped while the instance is `shorn`.
const COAT_CUBE_NAME: &str = "wool";
/// The multiply tint a fully-hurt mob flashes — dims green/blue toward red (a multiply
/// can't brighten, so this reads as a red cast rather than an additive glow).
const HURT_RED: [f32; 3] = [1.0, 0.25, 0.25];

/// The vertex tint for a mob flashing `hurt` (0..1): white at rest, fading toward
/// [`HURT_RED`] at full intensity.
fn hurt_tint(hurt: f32) -> [f32; 3] {
    let h = hurt.clamp(0.0, 1.0);
    [
        NO_TINT[0] + (HURT_RED[0] - NO_TINT[0]) * h,
        NO_TINT[1] + (HURT_RED[1] - NO_TINT[1]) * h,
        NO_TINT[2] + (HURT_RED[2] - NO_TINT[2]) * h,
    ]
}

/// Bake every instance of ONE species into `verts`/`indices` (cleared first, capacity
/// reused) using `model` at `scale`. Returns the index count. Each instance selects
/// its own animation — walk while moving, an `idle_*` if one is playing, else the
/// neutral rest pose — and (when the model has a `head` bone and the active animation
/// isn't already moving it) the AI head-look is applied to the head. The caller groups
/// instances by species and frustum-culls them first.
pub fn build_mob_instances(
    model: &Model,
    scale: f32,
    env: LightEnv,
    instances: &[MobRenderInstance],
    verts: &mut Vec<ItemVertex>,
    indices: &mut Vec<u32>,
) -> u32 {
    verts.clear();
    indices.clear();
    let head_bone = model.head_bone();
    let walk = model.animation("walk");
    for inst in instances {
        // Pose each bone. A dying mob uses a physics delta over the authored rest pose,
        // so static Blockbench group rotations are still present as it goes limp. A live
        // mob uses its animation (walk, a playing idle_*, else rest) plus AI head-look.
        let pose: Vec<Mat4> = if let Some(bones) = &inst.ragdoll {
            let rest = model.rest_pose();
            model
                .bones
                .iter()
                .enumerate()
                .map(|(b, bone)| match bones.get(b) {
                    Some(&(pos, rot)) => {
                        let rest_bone = rest.get(b).copied().unwrap_or(Mat4::IDENTITY);
                        let rest_pivot = rest_bone.transform_point3(bone.pivot);
                        Mat4::from_translation(pos)
                            * Mat4::from_quat(rot)
                            * Mat4::from_translation(-rest_pivot)
                            * rest_bone
                    }
                    None => rest.get(b).copied().unwrap_or(Mat4::IDENTITY),
                })
                .collect()
        } else {
            let anim: Option<&Animation> = if inst.moving {
                walk
            } else if let Some(i) = inst.idle_anim {
                model.idle_animation(i as usize)
            } else {
                None
            };
            let mut pose = match anim {
                Some(a) => model.pose(a, inst.anim_time),
                None => model.rest_pose(),
            };
            // Head-look: drive the head bone toward the AI's target unless the active
            // animation already moves the head (then it wins) or there's no head bone.
            if let Some(hb) = head_bone {
                let anim_drives_head = anim.is_some_and(|a| a.affects_bone(hb));
                if !anim_drives_head {
                    model.apply_head_look(&mut pose, hb, inst.head_yaw, inst.head_pitch);
                }
            }
            pose
        };

        // Place the posed model: scale to metres, yaw to facing, translate so model
        // `y=0` (the feet) sits at the instance position. For a ragdoll, `pos`/`yaw` are
        // frozen at death — only the bones move (within this `global`).
        let global = Mat4::from_translation(inst.pos)
            * Mat4::from_rotation_y(inst.yaw)
            * Mat4::from_scale(Vec3::splat(scale));
        // Two-channel RGB light folds into the tint (shade keeps the directional
        // term), so a mob standing in torch light stays lit at night.
        let rgb = light_rgb(
            DynLight {
                sky: inst.skylight,
                block: inst.blocklight,
            },
            env,
        );
        let hurt = hurt_tint(inst.hurt);
        let tint = [hurt[0] * rgb[0], hurt[1] * rgb[1], hurt[2] * rgb[2]];

        for cube in &model.cubes {
            if inst.shorn && cube.name == COAT_CUBE_NAME {
                continue;
            }
            let bone = pose.get(cube.bone).copied().unwrap_or(Mat4::IDENTITY);
            let s_cube = Mat4::from_translation(cube.origin)
                * Mat4::from_quat(euler_quat(cube.rotation))
                * Mat4::from_translation(-cube.origin);
            let m = global * bone * s_cube;

            for (slot, face) in Face::ALL.into_iter().enumerate() {
                let Some(uv) = cube.faces[slot] else { continue };
                push_face(verts, indices, m, face, cube.from, cube.to, uv, tint);
            }
        }
    }
    indices.len() as u32
}

/// Append one textured cube face (4 verts / 6 indices) transformed by `m`. Skips
/// degenerate (zero-area) faces — flat sub-cubes (legs/tail) have only one pair of
/// faces with area, and the rest collapse to lines.
#[allow(clippy::too_many_arguments)]
fn push_face(
    verts: &mut Vec<ItemVertex>,
    indices: &mut Vec<u32>,
    m: Mat4,
    face: Face,
    from: Vec3,
    to: Vec3,
    uv: [f32; 4],
    tint: [f32; 3],
) {
    let local = face_corners(face, from, to);
    let p: [Vec3; 4] = [
        m.transform_point3(Vec3::from(local[0])),
        m.transform_point3(Vec3::from(local[1])),
        m.transform_point3(Vec3::from(local[2])),
        m.transform_point3(Vec3::from(local[3])),
    ];
    if (p[1] - p[0]).cross(p[3] - p[0]).length_squared() < 1e-9 {
        return;
    }

    let shade = SHADES[face.shade_idx() as usize];
    // UV rect is [u0, v0_top, u1, v1_bottom]; assign per `quad_box` corner order
    // (p0 bottom-left, p1 bottom-right, p2 top-right, p3 top-left).
    let [u0, v0, u1, v1] = uv;
    let corner_uv = [[u0, v1], [u1, v1], [u1, v0], [u0, v0]];

    let start = verts.len() as u32;
    for i in 0..4 {
        verts.push(ItemVertex {
            pos: p[i].to_array(),
            uv: corner_uv[i],
            shade,
            tint,
        });
    }
    indices.extend_from_slice(&[start, start + 1, start + 2, start, start + 2, start + 3]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mob::Mob;

    fn owl_model() -> Model {
        let src = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/models/owl.bbmodel"
        ));
        Model::load(src).expect("owl model")
    }

    fn instance(anim_time: f32, moving: bool) -> MobRenderInstance {
        MobRenderInstance {
            kind: Mob::Owl,
            pos: Vec3::new(10.0, 64.0, -5.0),
            yaw: 0.0,
            anim_time,
            moving,
            idle_anim: None,
            head_yaw: 0.0,
            head_pitch: 0.0,
            skylight: 63,
            blocklight: 0,
            hurt: 0.0,
            shorn: false,
            ragdoll: None,
        }
    }

    #[test]
    fn empty_instances_produce_no_geometry() {
        let m = owl_model();
        let mut v = Vec::new();
        let mut i = Vec::new();
        assert_eq!(
            build_mob_instances(&m, 0.25, LightEnv::IDENTITY, &[], &mut v, &mut i),
            0
        );
        assert!(v.is_empty() && i.is_empty());
    }

    #[test]
    fn one_mob_bakes_quads_with_matched_indices() {
        let m = owl_model();
        let mut v = Vec::new();
        let mut i = Vec::new();
        let n = build_mob_instances(
            &m,
            0.25,
            LightEnv::IDENTITY,
            std::slice::from_ref(&instance(0.0, true)),
            &mut v,
            &mut i,
        );
        assert!(n > 0);
        assert_eq!(v.len() % 4, 0);
        assert_eq!(n as usize, i.len());
        assert_eq!(v.len() / 4 * 6, i.len());
        assert!(i.iter().all(|&ix| (ix as usize) < v.len()));
    }

    #[test]
    fn scale_sizes_the_baked_model() {
        let m = owl_model();
        let (mut v1, mut i1) = (Vec::new(), Vec::new());
        let (mut v2, mut i2) = (Vec::new(), Vec::new());
        build_mob_instances(
            &m,
            0.25,
            LightEnv::IDENTITY,
            std::slice::from_ref(&instance(0.0, false)),
            &mut v1,
            &mut i1,
        );
        build_mob_instances(
            &m,
            0.5,
            LightEnv::IDENTITY,
            std::slice::from_ref(&instance(0.0, false)),
            &mut v2,
            &mut i2,
        );
        // Same geometry, double scale -> double the vertical extent above the feet.
        let span = |v: &[ItemVertex]| v.iter().map(|x| x.pos[1]).fold(f32::MIN, f32::max) - 64.0;
        let (s1, s2) = (span(&v1), span(&v2));
        assert!(
            (s2 - s1 * 2.0).abs() < 1e-3,
            "scale should size the model: {s1} vs {s2}"
        );
    }

    #[test]
    fn moving_plays_walk_idle_uses_rest_pose() {
        // A moving mob at two phases differs (legs swing); two idle mobs are identical
        // regardless of anim_time (both render the rest pose).
        let m = owl_model();
        let bake = |t: f32, moving: bool| {
            let (mut v, mut i) = (Vec::new(), Vec::new());
            build_mob_instances(
                &m,
                0.25,
                LightEnv::IDENTITY,
                std::slice::from_ref(&instance(t, moving)),
                &mut v,
                &mut i,
            );
            v
        };
        let walk_a = bake(0.0, true);
        let walk_b = bake(0.25, true);
        let moved = walk_a
            .iter()
            .zip(&walk_b)
            .any(|(a, b)| (a.pos[2] - b.pos[2]).abs() > 1e-3);
        assert!(moved, "walking mob's legs move between phases");

        let rest_a = bake(0.0, false);
        let rest_b = bake(0.25, false);
        assert!(
            rest_a.iter().zip(&rest_b).all(|(a, b)| a.pos == b.pos),
            "idle mob ignores anim_time (always the rest pose)"
        );
    }

    #[test]
    fn shorn_hides_exactly_the_wool_named_cubes() {
        // A shorn sheep bakes without its `wool` cubes; a model with no wool-named
        // cubes (the owl) bakes identically shorn or not — proving the skip keys on
        // the authored cube name, not on the shorn flag alone.
        let sheep = Model::load(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/models/sheep.bbmodel"
        )))
        .expect("sheep model");
        assert!(
            sheep.cubes.iter().any(|c| c.name == COAT_CUBE_NAME),
            "fixture must author its fleece as `wool` cubes"
        );
        let bake = |model: &Model, kind: Mob, shorn: bool| {
            let mut inst = instance(0.0, false);
            inst.kind = kind;
            inst.shorn = shorn;
            let (mut v, mut i) = (Vec::new(), Vec::new());
            build_mob_instances(
                model,
                0.0625,
                LightEnv::IDENTITY,
                std::slice::from_ref(&inst),
                &mut v,
                &mut i,
            );
            v
        };
        let coated = bake(&sheep, Mob::Sheep, false);
        let shorn = bake(&sheep, Mob::Sheep, true);
        assert!(
            shorn.len() < coated.len(),
            "hiding the fleece removes geometry: {} -> {}",
            coated.len(),
            shorn.len()
        );

        let owl = owl_model();
        assert!(owl.cubes.iter().all(|c| c.name != COAT_CUBE_NAME));
        let owl_plain = bake(&owl, Mob::Owl, false);
        let owl_shorn = bake(&owl, Mob::Owl, true);
        assert_eq!(
            owl_plain.len(),
            owl_shorn.len(),
            "a model without wool cubes is unaffected by shorn"
        );
    }

    #[test]
    fn head_look_rotates_the_head_when_idle() {
        // Idle (rest pose, no head animation): a non-zero head_yaw must move the head
        // cubes, confirming head-look is wired into the bake.
        let m = owl_model();
        let bake = |head_yaw: f32| {
            let mut inst = instance(0.0, false);
            inst.head_yaw = head_yaw;
            let (mut v, mut i) = (Vec::new(), Vec::new());
            build_mob_instances(
                &m,
                0.25,
                LightEnv::IDENTITY,
                std::slice::from_ref(&inst),
                &mut v,
                &mut i,
            );
            v
        };
        let straight = bake(0.0);
        let turned = bake(1.0);
        assert!(
            straight.iter().zip(&turned).any(|(a, b)| a.pos != b.pos),
            "head-look should rotate the head when idle"
        );
    }
}
