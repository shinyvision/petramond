//! Third-person player body: the compiled `player.bbmodel` posed and baked each
//! frame into the mob-layout `ItemVertex` stream (world space, drawn in the mob
//! pass with the player's own skin texture bound).
//!
//! Pose composition, in order: the authored `walk` animation blended by
//! `walk_weight` (so starts/stops ease instead of snapping — [`Model::pose_layers`]),
//! the swing's body twist on the `body` bone, the head-look override on the
//! `head` bone (compensated for the twist so the gaze stays put), then the
//! held-arm attack swing COMPOSED onto the visual-right shoulder via
//! [`Model::apply_bone_rotation`] — so a punch layers over the walk cycle
//! instead of replacing it. The swing phase is the same
//! `HeldItemView::swing`/`swing_scale` state machine the first-person hand uses,
//! so mining sawtooths, breaks punch, and places jab identically in both views.
//! The swing curves are the reference biped attack swing (body yaw twist, the
//! quartic-eased arm raise with its look-pitch term, and the sine roll), with
//! signs mirrored for this model's facing.
//!
//! The model is authored front = −Z (the skin's face texture sits on the north
//! face), while engine yaw 0 faces +Z, so the body renders with `yaw + π`.

mod locomotion;

use glam::{Mat4, Quat, Vec3};

use super::item_model::ItemVertex;
use super::lighting::{fold_tint, DynLight, LightEnv};
use super::mob_model::{bake_model_cubes, hurt_tint};
use super::vanilla_swing::vanilla_swing;
use super::PlayerRenderInstance;
use petramond::player::model::{PLAYER_HIP_HEIGHT, PLAYER_MODEL_SCALE};
use petramond_world::bbmodel::Model;

/// The grip point in model pixels, in the visual-right arm's rest frame: centred
/// in the fist (the lower arm spans x 4..8, ends at y 12), a touch toward the
/// front. The authored model is rotated by π to face engine-forward; under this
/// engine's camera convention that makes the authored left arm the visual right
/// hand in third person.
const HAND_GRIP_PX: Vec3 = Vec3::new(6.0, 11.0, -1.5);
const HELD_SHOULDER_BONE: &str = "left_shoulder";
const HELD_ELBOW_BONE: &str = "left_elbow";
/// The OFF hand: the authored RIGHT arm lands on the visual LEFT side under
/// the same yaw+π handedness conversion that makes the authored left arm the
/// visual right. Its grip/attach transforms are the right hand's conjugated by
/// an arm-local X mirror ([`mirror_local`]), so the two fists stay symmetric
/// by construction.
const OFF_SHOULDER_BONE: &str = "right_shoulder";
const OFF_ELBOW_BONE: &str = "right_elbow";

/// Mirror an arm-local attach transform across the arm's YZ plane by
/// CONJUGATION: `S · M · S` with `S = diag(-1, 1, 1)`. Determinant preserved
/// (winding and texturing untouched) — used ONLY for the held block
/// mini-cube, whose symmetric geometry survives it AND whose stream draws on
/// the back-face-culled opaque pipeline (a true reflection would cull it
/// inside-out). Sprites and bbmodels use [`reflect_local`] instead.
fn mirror_local(m: Mat4) -> Mat4 {
    let s = Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0));
    s * m * s
}

/// TRUE reflection of an arm-local attach transform (`S · M`): the left fist
/// holds the MIRROR IMAGE of what the right fist holds — the same rule the
/// first-person pass uses (`hand::reflect_x`), and what Blockbench's
/// `thirdperson_lefthand` preview shows. Flips winding; the sprite and
/// bbmodel held streams draw on the double-sided mob pipeline, so that is
/// safe — the block mini-cube (opaque pipeline, back-face culled) must NOT
/// use this.
fn reflect_local(m: Mat4) -> Mat4 {
    Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)) * m
}

/// World-space size (blocks) of the held sprite-item slab.
const SPRITE_WORLD_SIZE: f32 = 0.60;
/// World-space size (blocks) of the held block mini-cube.
const BLOCK_WORLD_SIZE: f32 = 0.30;

/// How far the lying (sleeping) body's anchor floats above the mattress top:
/// half the 4 px body thickness plus a hair of clearance over the bed model.
const LIE_LIFT: f32 = 2.2 * PLAYER_MODEL_SCALE;

/// Bake the player body posed for this frame. Returns the emitted index count
/// plus the visual right- and left-hand world transforms (model-pixel units
/// under the placed, scaled body) for attaching the held items. `held` drives
/// the right arm's swing/eat channels, `off` the left arm's (its jab and its
/// off-hand eat) — both compose over the walk pose on their own shoulders.
pub(super) fn build_player_body(
    model: &Model,
    env: LightEnv,
    inst: &PlayerRenderInstance,
    bones: &[crate::BoneOffset],
    held: &crate::HeldItemView,
    off: &crate::HeldItemView,
    verts: &mut Vec<ItemVertex>,
    indices: &mut Vec<u32>,
) -> (u32, Mat4, Mat4) {
    let (swing, swing_scale, eat, eat_bob) = (held.swing, held.swing_scale, held.eat, held.eat_bob);
    verts.clear();
    indices.clear();

    let layers = locomotion::layers(model, inst);
    let mut pose = if layers.is_empty() {
        model.rest_pose()
    } else {
        model.pose_layers(&layers)
    };
    let head_animated = |hb: usize| layers.iter().any(|(a, _, _)| a.affects_bone(hb));

    // Asleep: the rest pose lying on its back — rotated flat about the feet,
    // head toward `body_yaw`, floated onto the mattress. Head-look and the arm
    // swing rest with it.
    if inst.sleeping {
        let global = Mat4::from_translation(inst.pos + Vec3::new(0.0, LIE_LIFT, 0.0))
            * Mat4::from_rotation_y(inst.body_yaw)
            * Mat4::from_rotation_x(std::f32::consts::FRAC_PI_2)
            * Mat4::from_scale(Vec3::splat(PLAYER_MODEL_SCALE));
        return bake_cubes(model, &pose, global, inst, env, verts, indices);
    }

    // Seated (riding a mob seat): thighs swing forward at the hip and the
    // shins hang back down from the knees — composed over the rest pose about
    // each bone's own pivot, so the exact leg geometry stays authored data.
    // The body, head-look, and arm channels below stay live: a rider looks
    // around and punches like anyone else. The bend is deliberately SHORT of
    // 90°: at a right angle the rotated thigh's top face lands coplanar with
    // the body cube's bottom and the pants z-fight (2026-07-15 playtest).
    const SEATED_HIP_BEND: f32 = 1.35; // ≈ 77°
    if inst.seated {
        for (hip, knee) in [("leftLeg", "left_knee"), ("rightLeg", "right_knee")] {
            if let Some(bone) = model.bone_named(hip) {
                // +X is limb-forward for the −Z-front biped (the zombie's
                // arms-forward rest pose uses the same sign).
                model.apply_bone_rotation(&mut pose, bone, Quat::from_rotation_x(SEATED_HIP_BEND));
            }
            if let Some(bone) = model.bone_named(knee) {
                model.apply_bone_rotation(&mut pose, bone, Quat::from_rotation_x(-SEATED_HIP_BEND));
            }
        }
    }

    // The vanilla third-person swing is the AUTHORED hand_player.json body
    // channel, played through the same bone machinery a pack's body curve
    // uses: verbatim for the MAIN hand (the authored left arm, visual right
    // under the yaw+π placement) and MIRRORED for the off jab (left_/right_
    // names swapped, the chirality channels negated — the same left-hand
    // rule every pose seam follows). Angles scale linearly with the hand's
    // swing amplitude, so a softer jab is a smaller arc, not another shape.
    let s = swing.clamp(0.0, 1.0);
    let off_s = off.swing.clamp(0.0, 1.0);
    let mut twist_total = 0.0;
    if swing > 0.0 {
        twist_total += play_swing_body(model, &mut pose, s, swing_scale, false);
    }
    if off.swing > 0.0 {
        twist_total += play_swing_body(model, &mut pose, off_s, off.swing_scale, true);
    }
    // The GAZE layers stay procedural on top of the data, deliberately: a
    // keyframe file cannot know where this viewer's player is looking.
    // Head-look compensates the twist the data put on the torso (the engine
    // is the sampler, so it knows), and the aim term leans each swinging
    // shoulder into the look pitch so a punch goes where the eyes do.
    if let Some(hb) = model.head_bone() {
        if !head_animated(hb) {
            model.apply_head_look(&mut pose, hb, inst.head_yaw - twist_total, inst.head_pitch);
            locomotion::stabilize_swim_gaze(model, &mut pose, hb, inst);
        }
    }
    if swing > 0.0 {
        if let Some(shoulder) = model.bone_named(HELD_SHOULDER_BONE) {
            let aim = (s * std::f32::consts::PI).sin() * (inst.head_pitch + 0.7) * 0.75;
            model.apply_bone_rotation(
                &mut pose,
                shoulder,
                Quat::from_rotation_x(aim * swing_scale),
            );
        }
    }
    if off.swing > 0.0 {
        if let Some(shoulder) = model.bone_named(OFF_SHOULDER_BONE) {
            let aim = (off_s * std::f32::consts::PI).sin() * (inst.head_pitch + 0.7) * 0.75;
            model.apply_bone_rotation(
                &mut pose,
                shoulder,
                Quat::from_rotation_x(aim * off.swing_scale),
            );
        }
    }
    // Eating: hold the forearm up so the food sits at the mouth (following the
    // gaze pitch like the swing does), bobbing with each bite. Blended by the
    // shared `eat` channel, so start/finish/abort ease exactly like first
    // person. Each hand's eat raises ITS OWN arm (the X raise is
    // mirror-symmetric, so the off arm needs no sign flips).
    if eat > 0.0 {
        if let Some(shoulder) = model.bone_named(HELD_SHOULDER_BONE) {
            let raise = 1.35 + (inst.head_pitch + 0.7) * 0.35;
            let rot = Quat::from_rotation_x(eat * (raise + eat_bob * 0.04));
            model.apply_bone_rotation(&mut pose, shoulder, rot);
        }
    }
    if off.eat > 0.0 {
        if let Some(shoulder) = model.bone_named(OFF_SHOULDER_BONE) {
            let raise = 1.35 + (inst.head_pitch + 0.7) * 0.35;
            let rot = Quat::from_rotation_x(off.eat * (raise + off.eat_bob * 0.04));
            model.apply_bone_rotation(&mut pose, shoulder, rot);
        }
    }
    // Claimed bone offsets LAST, so they compose on top of every engine layer
    // (walk, sneak, head-look, the swing and eat arm raises) rather than
    // fighting one. `apply_bone_offset` carries each through the bone's
    // descendants, so one shoulder offset raises the whole arm AND the item in
    // its fist — the held-pose seam never has to know.
    //
    // The engine's own layers are NOT claims, unlike the speed scale and the
    // barred actions, and the asymmetry is deliberate: a claim is replicated
    // authority, while an animation is derived presentation every viewer
    // computes for itself from a few replicated flags. Folding the walk cycle
    // into claims would put a sampled pose per bone per body on the wire to
    // buy nothing. That is exactly why `BonePoseMode::Replace` exists — a claim
    // needs a way to overrule a layer it cannot take part in.
    for offset in bones {
        let translation = Vec3::from(offset.translation) / 16.0 / PLAYER_MODEL_SCALE;
        if offset.hold {
            // A STANCE: the bone is held at rest + this rotation, discarding
            // the walk/sneak swing it would otherwise still be wearing
            // underneath. Degrees, added like an animation channel would.
            model.hold_bone(
                &mut pose,
                offset.bone,
                Vec3::from(offset.rotation),
                translation,
            );
        } else {
            model.apply_bone_offset(
                &mut pose,
                offset.bone,
                petramond_world::bbmodel::display_euler_quat(Vec3::from(offset.rotation)),
                translation,
            );
        }
    }

    // Authored front is −Z; engine yaw 0 faces +Z — hence the π. A seated
    // body leans with its mount: the whole rig tilts about the HIP pivot
    // (the point the seat carries — see `mob::riding::seat_world_pos`), so
    // the legs keep their authored place in the cart's own frame on a slope
    // instead of an upright body's knees driving through the tilted floor.
    let lean = if inst.seated && !inst.seat_tilt.is_level() {
        let hip = Vec3::new(0.0, PLAYER_HIP_HEIGHT, 0.0);
        Mat4::from_translation(hip) * inst.seat_tilt.rotation() * Mat4::from_translation(-hip)
    } else {
        Mat4::IDENTITY
    };
    let global = Mat4::from_translation(inst.pos)
        * Mat4::from_rotation_y(inst.body_yaw + std::f32::consts::PI)
        * lean
        * Mat4::from_scale(Vec3::splat(PLAYER_MODEL_SCALE));
    bake_cubes(model, &pose, global, inst, env, verts, indices)
}

/// Play one hand's share of the authored vanilla swing body channel
/// (`hand_player.json`) onto `pose` at `phase`, amplitude-scaled, and answer
/// the yaw (radians) it put on the torso — the head-look compensation's
/// input. `mirrored` plays the off hand's twin: `left`/`right` bone names
/// swapped and the chirality channels (rotation Y/Z, translation X) negated.
/// A missing/refused file swings nothing — rest.
fn play_swing_body(model: &Model, pose: &mut [Mat4], phase: f32, amp: f32, mirrored: bool) -> f32 {
    let swing = vanilla_swing();
    let Some(curve) = &swing.body else {
        return 0.0;
    };
    let mut torso_yaw = 0.0;
    for (at, (name, mode)) in curve.entries().iter().enumerate() {
        let (mut rot, mut trans) = curve.sample_entry(at, phase);
        for c in rot.iter_mut().chain(trans.iter_mut()) {
            *c *= amp;
        }
        let target: &str = if mirrored {
            rot[1] = -rot[1];
            rot[2] = -rot[2];
            trans[0] = -trans[0];
            // Names pre-swapped at load (`body_mirrored`), index-aligned.
            &swing.body_mirrored[at]
        } else {
            name
        };
        let Some(bone) = model.bone_named(target) else {
            continue;
        };
        let translation = Vec3::from(trans) / 16.0 / PLAYER_MODEL_SCALE;
        match mode {
            mod_api::BonePoseMode::Replace => {
                model.hold_bone(pose, bone, Vec3::from(rot), translation);
            }
            mod_api::BonePoseMode::Compose => {
                model.apply_bone_offset(
                    pose,
                    bone,
                    petramond_world::bbmodel::display_euler_quat(Vec3::from(rot)),
                    translation,
                );
            }
        }
        if target == "body" {
            torso_yaw += rot[1].to_radians();
        }
    }
    torso_yaw
}

/// Emit every cube of the posed model under `global`, lit and hurt-tinted, and
/// return the index count plus the visual right- and left-hand world
/// transforms.
fn bake_cubes(
    model: &Model,
    pose: &[Mat4],
    global: Mat4,
    inst: &PlayerRenderInstance,
    env: LightEnv,
    verts: &mut Vec<ItemVertex>,
    indices: &mut Vec<u32>,
) -> (u32, Mat4, Mat4) {
    let tint = fold_tint(
        hurt_tint(inst.hurt),
        DynLight::new(inst.skylight, inst.blocklight),
        env,
    );
    bake_model_cubes(model, pose, global, tint, |_| false, verts, indices);

    let arm = |elbow: &str, shoulder: &str| {
        let bone = model
            .bone_named(elbow)
            .or_else(|| model.bone_named(shoulder));
        global
            * bone
                .and_then(|b| pose.get(b).copied())
                .unwrap_or(Mat4::IDENTITY)
    };
    let hand = arm(HELD_ELBOW_BONE, HELD_SHOULDER_BONE);
    let off_hand = arm(OFF_ELBOW_BONE, OFF_SHOULDER_BONE);
    (indices.len() as u32, hand, off_hand)
}

/// Compose a claimed held pose ([`HeldItemView::pose`]) onto a hand attach
/// frame, once and upstream of the per-kind transforms, so EVERY held render
/// kind (block cube, extruded sprite, bbmodel) wears it identically.
///
/// The pose is authored in Blockbench display units (1/16-block pixels) and
/// `base_matrix` yields blocks, while the attach frames are in MODEL pixels;
/// conjugating by the body scale converts the translation and leaves the
/// rotation alone. The off-hand frame is the mirrored twin of the right, so
/// conjugating there negates the x-translation and the y/z rotations —
/// exactly [`DisplayTransform::left_hand`], reached without restating it.
///
/// [`DisplayTransform::left_hand`]: petramond_world::block_model::DisplayTransform::left_hand
pub(super) fn posed_hand(
    hand: Mat4,
    pose: &petramond_world::block_model::DisplayTransform,
    off_side: bool,
) -> Mat4 {
    if *pose == Default::default() {
        return hand;
    }
    let to_px = Mat4::from_scale(Vec3::splat(1.0 / PLAYER_MODEL_SCALE));
    let to_blocks = Mat4::from_scale(Vec3::splat(PLAYER_MODEL_SCALE));
    let local = to_px * pose.base_matrix() * to_blocks;
    if off_side {
        hand * mirror_local(local)
    } else {
        hand * local
    }
}

/// World transform for the EXTRUDED sprite item (unit XY slab). Tool art runs
/// diagonally (handle lower-left, head upper-right); rolling the art 55° in its
/// plane stands the tool along the sprite's +Y, the yaw turns the slab edge-on
/// (flat face to the sides), and the X tilt lays the tool axis pointing FORWARD
/// out of the fist with a slight rise. The sprite centre is then shifted along
/// that axis so the fist grips the HANDLE end, not the middle/head.
pub(super) fn held_sprite_transform(hand: Mat4) -> Mat4 {
    let size = SPRITE_WORLD_SIZE / PLAYER_MODEL_SCALE;
    let rot = Mat4::from_rotation_x(-65f32.to_radians())
        * Mat4::from_rotation_y(-std::f32::consts::FRAC_PI_2)
        * Mat4::from_rotation_z(55f32.to_radians());
    // The tool axis = the art diagonal carried through the pose; gripping ~30%
    // from the handle end pushes the centre forward along it.
    let axis = rot.transform_vector3(Vec3::new(
        std::f32::consts::FRAC_1_SQRT_2,
        std::f32::consts::FRAC_1_SQRT_2,
        0.0,
    ));
    hand * Mat4::from_translation(HAND_GRIP_PX + axis * (0.30 * size))
        * rot
        * Mat4::from_scale(Vec3::splat(size))
}

/// World transform for a held block mini-cube (built origin-centred, unit size):
/// a corner turned toward the front, floated just ahead of the fist.
pub(super) fn held_block_transform(hand: Mat4) -> Mat4 {
    hand * Mat4::from_translation(HAND_GRIP_PX + Vec3::new(0.0, -0.5, -2.0))
        * Mat4::from_rotation_y(std::f32::consts::FRAC_PI_4)
        * Mat4::from_scale(Vec3::splat(BLOCK_WORLD_SIZE / PLAYER_MODEL_SCALE))
}

/// World transform for a held bbmodel item: the authored `thirdperson_righthand`
/// display pose (rotation/translation/scale straight from the `.bbmodel`),
/// composed under the hand-layer frame exactly like the first-person path uses
/// `firstperson_righthand` — display "up" points forward out of the fist, one
/// display unit is one world block, and the authored pose does the rest. A model
/// that sits wrong in hand has an untuned `thirdperson_righthand` pose; tune it
/// in Blockbench, not here.
///
/// The reorientation is `Rx(-90°)` and NOTHING ELSE. It carried an extra
/// `Ry(180°)` until 2026-08-22, which turned every bbmodel item end-over-end in
/// the fist relative to its own Blockbench preview — the game showing something
/// the model does not say.
///
/// It was nearly invisible because the only bbmodel items were the buckets,
/// which are four-fold symmetric about the axis it flipped; it surfaced the
/// moment an item with a top and a bottom went in a hand. DO NOT "fix" a
/// mis-oriented hold by turning the asset: that makes the `.bbmodel` lie, and
/// every other consumer of the model inherits the lie. When the game and
/// Blockbench disagree about a model, the GAME is wrong.
pub(super) fn held_model_transform(
    hand: Mat4,
    kind: petramond_world::block_model::BlockModelKind,
) -> Mat4 {
    let pose = &petramond_world::block_model::display(kind).thirdperson_righthand;
    hand * Mat4::from_translation(HAND_GRIP_PX)
        * Mat4::from_scale(Vec3::splat(1.0 / PLAYER_MODEL_SCALE))
        * Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2)
        * pose.base_matrix()
        * petramond_world::block_model::instance(kind).display_from_unit
}

/// OFF-hand (left fist) twins of the three attach transforms. The sprite and
/// bbmodel twins are the right hand's composition truly REFLECTED
/// ([`reflect_local`]) — the left fist holds the mirror image, so a tool's
/// head still points forward and a model shows the same face its Blockbench
/// lefthand preview shows. The block mini-cube keeps the winding-preserving
/// conjugation (its pipeline culls, and a cube's three-quarter view survives
/// conjugation).
pub(super) fn held_sprite_transform_off(off_hand: Mat4) -> Mat4 {
    let size = SPRITE_WORLD_SIZE / PLAYER_MODEL_SCALE;
    let rot = Mat4::from_rotation_x(-65f32.to_radians())
        * Mat4::from_rotation_y(-std::f32::consts::FRAC_PI_2)
        * Mat4::from_rotation_z(55f32.to_radians());
    let axis = rot.transform_vector3(Vec3::new(
        std::f32::consts::FRAC_1_SQRT_2,
        std::f32::consts::FRAC_1_SQRT_2,
        0.0,
    ));
    off_hand
        * reflect_local(
            Mat4::from_translation(HAND_GRIP_PX + axis * (0.30 * size))
                * rot
                * Mat4::from_scale(Vec3::splat(size)),
        )
}

pub(super) fn held_block_transform_off(off_hand: Mat4) -> Mat4 {
    off_hand
        * mirror_local(
            Mat4::from_translation(HAND_GRIP_PX + Vec3::new(0.0, -0.5, -2.0))
                * Mat4::from_rotation_y(std::f32::consts::FRAC_PI_4)
                * Mat4::from_scale(Vec3::splat(BLOCK_WORLD_SIZE / PLAYER_MODEL_SCALE)),
        )
}

/// The bbmodel off-hand attach — the third-person twin of the first-person
/// rule (`hand::held_model_off`, Blockbench's lefthand composition): the
/// hand-layer FRAME mirrors by conjugation (`mirror_local` — for this frame
/// that is just the grip's x negated: an x-rotation is mirror-symmetric), the
/// pose is the slot's values with `translation.x` /
/// `rotation.y` / `rotation.z` negated ([`DisplayTransform::left_hand`],
/// authored `thirdperson_lefthand` included), and the geometry + its
/// `display_from_unit` rebase stay untouched — no reflection.
pub(super) fn held_model_transform_off(
    off_hand: Mat4,
    kind: petramond_world::block_model::BlockModelKind,
) -> Mat4 {
    let display = petramond_world::block_model::display(kind);
    let pose = display
        .thirdperson_lefthand
        .as_ref()
        .unwrap_or(&display.thirdperson_righthand)
        .left_hand();
    off_hand
        * mirror_local(
            Mat4::from_translation(HAND_GRIP_PX)
                * Mat4::from_scale(Vec3::splat(1.0 / PLAYER_MODEL_SCALE))
                * Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2),
        )
        * pose.base_matrix()
        * petramond_world::block_model::instance(kind).display_from_unit
}

/// CPU-transform the given vertex positions by `m` — baking in model space then
/// placing in the world on the CPU, since the opaque pipeline has no per-draw
/// model matrix. Takes a position iterator so both vertex layouts (packed
/// [`petramond_mesh::Vertex`] and explicit-UV [`ItemVertex`]) share it.
pub(super) fn transform_positions<'a>(pos: impl Iterator<Item = &'a mut [f32; 3]>, m: Mat4) {
    for p in pos {
        *p = m.transform_point3(Vec3::from(*p)).to_array();
    }
}

#[cfg(test)]
mod tests;
