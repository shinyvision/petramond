//! World-space geometry for animated entity models (mobs), baked each frame into
//! the explicit-UV [`ItemVertex`] stream and drawn by
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
use super::lighting::{fold_tint_self_lit, mul3, DynLight, LightEnv};
use super::MobRenderInstance;
use petramond_math::face::Face;
use petramond_mesh::SHADES;
use petramond_world::bbmodel::{clips, euler_quat, face_corners, Animation, Cube, Model};

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
/// [`HURT_RED`] at full intensity. Shared with the third-person player body so
/// taking damage flashes the player exactly like a hurt mob.
pub(super) fn hurt_tint(hurt: f32) -> [f32; 3] {
    let h = hurt.clamp(0.0, 1.0);
    [
        NO_TINT[0] + (HURT_RED[0] - NO_TINT[0]) * h,
        NO_TINT[1] + (HURT_RED[1] - NO_TINT[1]) * h,
        NO_TINT[2] + (HURT_RED[2] - NO_TINT[2]) * h,
    ]
}

/// A body's baked vertex tint — mobs and player bodies alike: the hurt flash
/// times its emitter tint, lit by the sampled light mixed toward full bright by
/// its emitter self-lighting.
pub(super) fn body_tint(
    hurt: f32,
    emitter_tint: [f32; 3],
    light: DynLight,
    env: LightEnv,
    self_lit: f32,
) -> [f32; 3] {
    fold_tint_self_lit(mul3(hurt_tint(hurt), emitter_tint), light, env, self_lit)
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
    let walk = model.animation(clips::WALK);
    // Animation layers for the instance being posed (base + active named
    // anims), reused across instances.
    let mut layers: Vec<(&Animation, f32, f32)> = Vec::new();
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
            // Base animation (walk while moving, a playing idle_*, else none)
            // plus every active NAMED animation at its replicated self-clocked
            // phase. Names the model lacks are skipped, like disabled pack
            // content.
            let base: Option<&Animation> = if inst.moving {
                walk
            } else if let Some(i) = inst.idle_anim {
                model.idle_animation(i as usize)
            } else {
                None
            };
            layers.clear();
            layers.extend(base.map(|a| (a, inst.anim_time, 1.0)));
            layers.extend(
                inst.anims
                    .iter()
                    .filter(|(_, _, weight)| *weight > 0.001)
                    .filter_map(|(name, phase, weight)| {
                        model.animation(name).map(|a| (a, *phase, *weight))
                    }),
            );
            let looking_head =
                head_bone.filter(|&hb| !layers.iter().any(|(a, _, _)| a.affects_bone(hb)));
            if inst.hurt > 0.001 {
                if let Some(hurt) = model.animation(clips::HURT) {
                    layers.push((
                        hurt,
                        (1.0 - inst.hurt.clamp(0.0, 1.0)) * hurt.length,
                        inst.hurt.clamp(0.0, 1.0),
                    ));
                }
            }
            let mut pose = if layers.is_empty() {
                model.rest_pose()
            } else {
                model.pose_layers(&layers)
            };
            // Hurt is additive to the gaze; authored actions still own the head.
            if let Some(hb) = looking_head {
                let parent = model.bones[hb]
                    .parent
                    .map(|p| pose[p].to_scale_rotation_translation().1)
                    .unwrap_or(glam::Quat::IDENTITY);
                let look = glam::Quat::from_rotation_y(inst.head_yaw)
                    * glam::Quat::from_rotation_x(inst.head_pitch);
                model.apply_bone_rotation(&mut pose, hb, parent * look * parent.conjugate());
            }
            pose
        };

        // Place the posed model: scale to metres, into the body frame (yaw to
        // facing, the tilt inside it), translate so model `y=0` (the feet)
        // sits at the instance position. For a ragdoll, `pos`/`yaw` are
        // frozen at death — only the bones move (within this `global`).
        let global = Mat4::from_translation(inst.pos)
            * inst.tilt.body_frame(inst.yaw)
            * Mat4::from_scale(Vec3::splat(scale));
        // Two-channel RGB light folds into the tint (shade keeps the directional
        // term), so a mob standing in torch light stays lit at night.
        let tint = body_tint(
            inst.hurt,
            inst.emitter_tint,
            DynLight::new(inst.skylight, inst.blocklight),
            env,
            inst.emitter_self_lit,
        );
        bake_model_cubes(
            model,
            &pose,
            global,
            tint,
            |cube| inst.shorn && cube.name == COAT_CUBE_NAME,
            verts,
            indices,
        );
    }
    indices.len() as u32
}

/// Emit every cube of the posed model under `global`, tinted by `tint`, skipping
/// cubes where `skip` returns true (per-instance part hiding like a shorn sheep's
/// `wool` cubes; pass `|_| false` for none). Per cube the transform is
/// `global · pose[bone] · S_cube` (see the module doc). Shared with the
/// third-person player bake ([`super::player_model`]).
pub(super) fn bake_model_cubes(
    model: &Model,
    pose: &[Mat4],
    global: Mat4,
    tint: [f32; 3],
    skip: impl Fn(&Cube) -> bool,
    verts: &mut Vec<ItemVertex>,
    indices: &mut Vec<u32>,
) {
    for cube in &model.cubes {
        if skip(cube) {
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

/// Append one textured cube face (4 verts / 6 indices) transformed by `m`. Skips
/// degenerate (zero-area) faces — flat sub-cubes (legs/tail) have only one pair of
/// faces with area, and the rest collapse to lines. Shared with the third-person
/// player bake ([`super::player_model`]).
#[allow(clippy::too_many_arguments)]
pub(super) fn push_face(
    verts: &mut Vec<ItemVertex>,
    indices: &mut Vec<u32>,
    m: Mat4,
    face: Face,
    from: Vec3,
    to: Vec3,
    uv: petramond_world::bbmodel::FaceUv,
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
    // Corner UVs in `quad_box` order (p0 bottom-left, p1 bottom-right, p2
    // top-right, p3 top-left), per-face rotation applied.
    let corner_uv = uv.corner_uv();

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
mod tests;
