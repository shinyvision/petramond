//! Animated entity models (mobs): each instance's skeleton posed per frame on
//! the CPU and handed to the GPU skinning path (`crate::skinned`), drawn by
//! the `skinned` pipeline (see `pipeline.rs` / `skinned.wgsl`).
//!
//! Generic over species: the caller passes the parsed [`Model`], its render `scale`,
//! and (optionally) the walk [`Animation`]; each instance is posed by its own
//! `anim_time` when `moving`, or in the model's neutral [rest pose](Model::rest_pose)
//! when idle (so a standing mob shows straight legs with no per-animation tuning).
//!
//! Per cube the world transform is `G · pose[bone] · S_cube`, where `S_cube` is
//! the cube's modelled static tilt, `pose[bone]` the animation (or rest)
//! transform, and `G = T(pos)·yaw·scale` places the model in the world. The
//! species. `SkinMesh` holds every face under `S_cube` once; per frame a mob
//! costs one palette entry `G · pose[bone]` per bone and one instance row —
//! never a vertex. Faces use the same `quad_box` winding the chunk mesher +
//! block models use, so the bbmodel's per-face sub-rect UVs map upright.
//!
//! `bake_model_cubes` still bakes a posed model into world-space vertices on
//! the CPU: the first-person rig (one rig, drawn through the hand pass's own
//! MVP) uses it, and it is the reference the skinned mesh is pinned against.

use std::sync::Arc;

use glam::{Mat4, Vec3};

use super::item_model::ItemVertex;
use super::lighting::{fold_tint_self_lit, mul3, DynLight, LightEnv};
use super::skinned::{bone_slots, SkinBatch, SkinLook, SkinMesh, PART_COAT};
use super::MobRenderInstance;
use petramond_math::face::Face;
use petramond_mesh::face::FaceShading;
use petramond_mesh::SHADES;
use petramond_world::bbmodel::{clips, euler_quat, face_corners, Animation, Model};

/// White: mobs are textured directly (no foliage tint), so the shader's
/// `tex.rgb * shade * tint` reduces to `tex.rgb * shade`.
const NO_TINT: [f32; 3] = [1.0, 1.0, 1.0];
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

/// A body's lit tint: the hurt flash times its emitter tint, lit by the
/// sampled light mixed toward full bright by its emitter self-lighting. The
/// first-person arms bake it into their vertices; skinned bodies carry the
/// same inputs per instance and `skinned.wgsl` computes the same value.
pub(super) fn body_tint(
    hurt: f32,
    emitter_tint: [f32; 3],
    light: DynLight,
    env: LightEnv,
    self_lit: f32,
) -> [f32; 3] {
    fold_tint_self_lit(mul3(hurt_tint(hurt), emitter_tint), light, env, self_lit)
}

/// Where one posed mob holds one item: the hand's grip and the light to draw
/// the item in. Collected while the mobs bake and drawn with the held items.
pub(crate) struct MobHeld {
    pub grip: crate::player_model::Grip,
    pub item: petramond_world::item::ItemType,
    pub off_side: bool,
    pub light: DynLight,
}

/// What a species' model resolves to once, so the per-instance pose compares
/// no names: its hand bones, its coat cubes, and its rest-pose self-AO.
pub(crate) struct MobRig {
    /// Main and off hand: the bone index and the grip point in its rest pose.
    hands: [Option<(usize, Vec3)>; 2],
    hand_roll: f32,
    /// Per cube: part of the shearable coat.
    coat: Vec<bool>,
    /// Rest-pose self-AO per cube face corner, at the species' strength.
    self_ao: Option<Vec<[[f32; 4]; 6]>>,
}

impl MobRig {
    /// `coat` names the species' shearable-coat cubes, when it has a coat.
    pub(crate) fn resolve(
        model: &Model,
        hands: Option<petramond::mob::MobHands>,
        coat: Option<&str>,
    ) -> Self {
        let hand = |hand: Option<(&str, [f32; 3])>| {
            let (bone, grip) = hand?;
            let index = model.bones.iter().position(|b| b.name == bone)?;
            Some((index, Vec3::from_array(grip)))
        };
        Self {
            hands: [hand(hands.map(|h| h.main)), hand(hands.and_then(|h| h.off))],
            hand_roll: hands.map_or(0.0, |h| h.roll),
            coat: model
                .cubes
                .iter()
                .map(|cube| Some(cube.name.as_str()) == coat)
                .collect(),
            self_ao: None,
        }
    }

    /// Add the rest-pose self-AO table at `strength`. A coat casts nothing:
    /// shorn, the body it covered must not stay dark.
    pub(crate) fn with_self_ao(mut self, model: &Model, scale: f32, strength: f32) -> Self {
        let world_px = 1.0 / (16.0 * scale.max(1e-6));
        let mut table = model.rest_self_ao(world_px, |cube| !self.coat[cube]);
        for ao in table.iter_mut().flatten().flatten() {
            *ao = 1.0 - strength * (1.0 - *ao);
        }
        self.self_ao = Some(table);
        self
    }

    /// The species' static skinned mesh at render `scale`: every cube face in
    /// bind space, the coat's cubes in [`PART_COAT`] so a shorn instance can
    /// hide them, the self-AO folded into the shade.
    pub(crate) fn mesh(&self, model: &Model, scale: f32) -> SkinMesh {
        SkinMesh::build(model, scale, self.self_ao.as_deref(), |cube| {
            if self.coat.get(cube).copied().unwrap_or(false) {
                PART_COAT
            } else {
                0
            }
        })
    }
}

/// The frame's mob arena rows and the session's animation-name table, as the
/// pose reads them: every [`MobRenderInstance`] addresses its fading gaits,
/// named layers and ragdoll bones by range into `arena`.
#[derive(Copy, Clone)]
pub(crate) struct MobLayers<'a> {
    pub arena: &'a crate::MobArena,
    pub names: &'a crate::AnimNames,
}

/// One species' animation ids resolved to its model's clips. An id resolves
/// by name once; after that a lookup is an index plus a pointer compare
/// against the table's name (a new session's table names ids afresh, which
/// the compare catches), so no frame hashes or compares a clip name.
pub(crate) struct AnimClips<'m> {
    slots: Vec<Option<(Arc<str>, Option<&'m Animation>)>>,
}

impl Default for AnimClips<'_> {
    fn default() -> Self {
        Self { slots: Vec::new() }
    }
}

impl<'m> AnimClips<'m> {
    /// `id`'s clip in `model`, or `None` when the model has no such clip (or
    /// the id is not in `names`).
    pub(crate) fn resolve(
        &mut self,
        model: &'m Model,
        names: &crate::AnimNames,
        id: crate::AnimId,
    ) -> Option<&'m Animation> {
        let name = names.get(id)?;
        let i = id.0 as usize;
        if self.slots.len() <= i {
            self.slots.resize(i + 1, None);
        }
        match &self.slots[i] {
            Some((cached, clip)) if Arc::ptr_eq(cached, name) => *clip,
            _ => {
                let clip = model.animation(name);
                self.slots[i] = Some((Arc::clone(name), clip));
                clip
            }
        }
    }
}

/// What posing one species keeps between frames: its resolved clips, and the
/// per-instance layer list's storage.
pub(crate) struct MobPoseCache<'m> {
    clips: AnimClips<'m>,
    layers: Vec<(&'m Animation, f32, f32)>,
}

impl Default for MobPoseCache<'_> {
    fn default() -> Self {
        Self {
            clips: AnimClips::default(),
            layers: Vec::new(),
        }
    }
}

/// Pose every instance of ONE species (`model` at `scale`) into `batch` —
/// one palette run and one instance row each, contiguous — and answer the
/// instance range the species draws. Each instance selects its own animation
/// — walk while moving, an `idle_*` if one is playing, else the neutral rest
/// pose — and (when the model has a `head` bone and the active animation isn't
/// already moving it) the AI head-look is applied to the head. The caller
/// groups instances by species and frustum-culls them first.
pub(crate) fn pose_mob_instances<'i, 'm>(
    model: &'m Model,
    scale: f32,
    instances: impl IntoIterator<Item = &'i MobRenderInstance>,
    frame: MobLayers<'_>,
    render_origin: glam::IVec3,
    rig: &MobRig,
    cache: &mut MobPoseCache<'m>,
    batch: &mut SkinBatch,
    held: &mut Vec<MobHeld>,
) -> std::ops::Range<u32> {
    let first = batch.next_instance();
    let slots = bone_slots(model);
    let head_bone = model.head_bone();
    let walk = model.animation(clips::WALK);
    let hurt_clip = model.animation(clips::HURT);
    // Animation layers for the instance being posed (base + active named
    // anims), reused across instances and frames.
    let MobPoseCache {
        clips: anim_clips,
        layers,
    } = cache;
    for inst in instances {
        // Pose each bone. A dying mob uses a physics delta over the authored rest pose,
        // so static Blockbench group rotations are still present as it goes limp. A live
        // mob uses its animation (walk, a playing idle_*, else rest) plus AI head-look.
        let pose: Vec<Mat4> = if let Some(bones) = inst.ragdoll {
            let bones = bones.of(&frame.arena.ragdoll);
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
            layers.extend(base.map(|a| (a, inst.anim_time, inst.gait_weight)));
            layers.extend(
                inst.gait_fades
                    .of(&frame.arena.gait_fades)
                    .iter()
                    .filter_map(|fade| {
                        let anim = match fade.clip {
                            crate::GaitClip::Walk => walk,
                            crate::GaitClip::Idle(i) => model.idle_animation(i as usize),
                        };
                        anim.map(|a| (a, fade.phase, fade.weight))
                    }),
            );
            layers.extend(
                inst.anims
                    .of(&frame.arena.anims)
                    .iter()
                    .filter(|layer| layer.weight > 0.001)
                    .filter_map(|layer| {
                        anim_clips
                            .resolve(model, frame.names, layer.anim)
                            .map(|a| (a, layer.phase, layer.weight))
                    }),
            );
            // Clips that move the head own it by their WEIGHT: a walk easing
            // in hands the head over gradually, never in one frame.
            let looking_head = head_bone.map(|hb| {
                let owned: f32 = layers
                    .iter()
                    .filter(|(a, _, _)| a.affects_bone(hb))
                    .map(|(_, _, weight)| weight.clamp(0.0, 1.0))
                    .sum();
                (hb, 1.0 - owned.min(1.0))
            });
            if inst.hurt > 0.001 {
                if let Some(hurt) = hurt_clip {
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
                model.pose_layers(layers)
            };
            // Hurt is additive to the gaze; authored actions still own the head.
            if let Some((hb, free)) = looking_head.filter(|(_, free)| *free > 0.001) {
                let parent = model.bones[hb]
                    .parent
                    .map(|p| pose[p].to_scale_rotation_translation().1)
                    .unwrap_or(glam::Quat::IDENTITY);
                let look = glam::Quat::IDENTITY.slerp(
                    glam::Quat::from_rotation_y(inst.head_yaw)
                        * glam::Quat::from_rotation_x(inst.head_pitch),
                    free,
                );
                model.apply_bone_rotation(&mut pose, hb, parent * look * parent.conjugate());
            }
            pose
        };

        // Place the posed model: scale to metres, into the body frame (yaw to
        // facing, the tilt inside it), translate so model `y=0` (the feet)
        // sits at the instance position. For a ragdoll, `pos`/`yaw` are
        // frozen at death — only the bones move (within this `global`).
        let global = Mat4::from_translation(inst.pos.relative_to(render_origin))
            * inst.tilt.body_frame(inst.yaw)
            * Mat4::from_scale(Vec3::splat(scale));
        // The two-channel RGB light rides the instance and lights the tint in
        // the shader (shade keeps the directional term), so a mob standing in
        // torch light stays lit at night. A shorn mob hides its coat.
        batch.push(
            &pose,
            global,
            slots,
            SkinLook {
                hurt: inst.hurt,
                emitter_tint: inst.emitter_tint,
                emitter_self_lit: inst.emitter_self_lit,
                light: DynLight::new(inst.skylight, inst.blocklight),
                hidden: if inst.shorn { PART_COAT } else { 0 },
            },
        );
        for (side, hand) in rig.hands.iter().enumerate() {
            let (Some((bone, grip)), Some(item)) = (*hand, inst.held[side]) else {
                continue;
            };
            let quarter_back = if item.sprite_face_leads() {
                std::f32::consts::FRAC_PI_2
            } else {
                0.0
            };
            held.push(MobHeld {
                grip: crate::player_model::Grip {
                    frame: global * pose.get(bone).copied().unwrap_or(Mat4::IDENTITY),
                    point: grip,
                    px: scale,
                    roll: rig.hand_roll + quarter_back,
                },
                item,
                off_side: side == 1,
                light: DynLight::new(inst.skylight, inst.blocklight),
            });
        }
    }
    first..batch.next_instance()
}

/// Emit every cube of the posed model under `global`, tinted by `tint`, skipping
/// cubes whose INDEX `skip` returns true for (per-instance part hiding like a
/// shorn sheep's coat; pass `|_| false` for none). Per cube the transform is
/// `global · pose[bone] · S_cube` (see the module doc). The first-person rig's
/// CPU bake ([`super::first_person`]), and the reference [`SkinMesh`] is
/// tested against.
#[allow(clippy::too_many_arguments)]
pub(super) fn bake_model_cubes(
    model: &Model,
    pose: &[Mat4],
    global: Mat4,
    tint: [f32; 3],
    skip: impl Fn(usize) -> bool,
    self_ao: Option<&[[[f32; 4]; 6]]>,
    verts: &mut Vec<ItemVertex>,
    indices: &mut Vec<u32>,
) {
    for (ci, cube) in model.cubes.iter().enumerate() {
        if skip(ci) {
            continue;
        }
        let bone = pose.get(cube.bone).copied().unwrap_or(Mat4::IDENTITY);
        let s_cube = Mat4::from_translation(cube.origin)
            * Mat4::from_quat(euler_quat(cube.rotation))
            * Mat4::from_translation(-cube.origin);
        let m = global * bone * s_cube;

        for (slot, face) in Face::ALL.into_iter().enumerate() {
            let Some(uv) = cube.faces[slot] else { continue };
            let ao = self_ao
                .and_then(|table| table.get(ci))
                .map_or([1.0; 4], |faces| faces[slot]);
            push_face(verts, indices, m, face, cube.from, cube.to, uv, tint, ao);
        }
    }
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
    uv: petramond_world::bbmodel::FaceUv,
    tint: [f32; 3],
    ao: [f32; 4],
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
            shade: shade * ao[i],
            tint,
        });
    }
    indices.extend(
        petramond_world::block_model::model_face_tris(ao)
            .into_iter()
            .map(|i| start + i),
    );
}

#[cfg(test)]
mod tests;
