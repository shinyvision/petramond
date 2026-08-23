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

use glam::{Mat4, Quat, Vec3};

use super::item_model::ItemVertex;
use super::lighting::{fold_tint, DynLight, LightEnv};
use super::mob_model::{bake_model_cubes, hurt_tint};
use super::PlayerRenderInstance;
use petramond::player::model::PLAYER_MODEL_SCALE;
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

    // Locomotion pose: a cross-fade of up to three layers of the two authored
    // clips. Upright movement plays `walk`; sneaking swaps in the `sneak` clip —
    // its FRAME 0 is the standing crouch stance, so a still sneaker holds
    // `sneak@0` at full weight, a moving sneaker plays the clip's own cycle, and
    // the walk blend (`walk_weight`) cross-fades between those two exactly like
    // it fades walk↔rest when upright. Weights sum to ≤ 1; the remainder is the
    // rest pose (`pose_layers` scales toward rest).
    let sneak = model.animation("sneak");
    let sw = if inst.sleeping || inst.seated || sneak.is_none() {
        0.0
    } else {
        inst.sneak_weight.clamp(0.0, 1.0)
    };
    let ww = if inst.sleeping || inst.seated {
        0.0
    } else {
        inst.walk_weight.clamp(0.0, 1.0)
    };
    let mut layers: Vec<(&petramond_world::bbmodel::Animation, f32, f32)> = Vec::with_capacity(3);
    if let Some(walk) = model.animation("walk") {
        if ww * (1.0 - sw) > 0.001 {
            layers.push((walk, inst.anim_time, ww * (1.0 - sw)));
        }
    }
    if let Some(sneak) = sneak {
        if sw * ww > 0.001 {
            layers.push((sneak, inst.anim_time, sw * ww));
        }
        if sw * (1.0 - ww) > 0.001 {
            layers.push((sneak, 0.0, sw * (1.0 - ww)));
        }
    }
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

    // Reference biped attack swing, mirrored for this model's −Z front: the body
    // twists with the punch, the head compensates so the gaze stays fixed, and
    // the arm raise composes over whatever the walk pose put on the shoulder.
    let s = swing.clamp(0.0, 1.0);
    // Negative: the twist must wind the HELD (visual-right) shoulder back then
    // drive it forward; like the roll below, it mirrors with the arm swap.
    let twist = if swing > 0.0 {
        (s.sqrt() * std::f32::consts::TAU).sin() * -0.2 * swing_scale
    } else {
        0.0
    };
    // The LEFT arm's jab twists the torso the OTHER way (its chirality terms
    // are the right arm's negated). The two compose additively — a same-frame
    // main swing + off jab is rare, and each stays a small angle.
    let off_s = off.swing.clamp(0.0, 1.0);
    let off_twist = if off.swing > 0.0 {
        (off_s.sqrt() * std::f32::consts::TAU).sin() * 0.2 * off.swing_scale
    } else {
        0.0
    };
    let twist_total = twist + off_twist;
    if twist_total != 0.0 {
        if let Some(body) = model.bone_named("body") {
            model.apply_bone_rotation(&mut pose, body, Quat::from_rotation_y(twist_total));
        }
    }
    if let Some(hb) = model.head_bone() {
        if !head_animated(hb) {
            model.apply_head_look(&mut pose, hb, inst.head_yaw - twist_total, inst.head_pitch);
        }
    }
    if swing > 0.0 {
        if let Some(shoulder) = model.bone_named(HELD_SHOULDER_BONE) {
            // Quartic-eased raise + the look-pitch term, then the arm follows the
            // body twist at 2× total (1× inherited from the body bone + 1× here).
            let eased = 1.0 - (1.0 - s).powi(4);
            let raise = (eased * std::f32::consts::PI).sin() * 1.2;
            let pitch_term = (s * std::f32::consts::PI).sin() * (inst.head_pitch + 0.7) * 0.75;
            let roll = (s * std::f32::consts::PI).sin() * 0.4;
            // The visual right arm is the authored left arm after the yaw+π
            // placement, so the shoulder roll mirrors the authored-right swing.
            let rot = Quat::from_rotation_x((raise + pitch_term) * swing_scale)
                * Quat::from_rotation_y(twist)
                * Quat::from_rotation_z(-roll * swing_scale);
            model.apply_bone_rotation(&mut pose, shoulder, rot);
        }
    }
    // The LEFT arm's jab: the same raise (X terms are mirror-symmetric), the
    // chirality-carrying Y/Z terms negated for the mirrored arm.
    if off.swing > 0.0 {
        if let Some(shoulder) = model.bone_named(OFF_SHOULDER_BONE) {
            let eased = 1.0 - (1.0 - off_s).powi(4);
            let raise = (eased * std::f32::consts::PI).sin() * 1.2;
            let pitch_term = (off_s * std::f32::consts::PI).sin() * (inst.head_pitch + 0.7) * 0.75;
            let roll = (off_s * std::f32::consts::PI).sin() * 0.4;
            let rot = Quat::from_rotation_x((raise + pitch_term) * off.swing_scale)
                * Quat::from_rotation_y(off_twist)
                * Quat::from_rotation_z(roll * off.swing_scale);
            model.apply_bone_rotation(&mut pose, shoulder, rot);
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
                petramond_world::bbmodel::euler_quat(Vec3::from(offset.rotation)),
                translation,
            );
        }
    }

    // Authored front is −Z; engine yaw 0 faces +Z — hence the π.
    let global = Mat4::from_translation(inst.pos)
        * Mat4::from_rotation_y(inst.body_yaw + std::f32::consts::PI)
        * Mat4::from_scale(Vec3::splat(PLAYER_MODEL_SCALE));
    bake_cubes(model, &pose, global, inst, env, verts, indices)
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
mod tests {
    use super::*;
    use petramond::player::model::player_model;

    fn instance() -> PlayerRenderInstance {
        PlayerRenderInstance {
            pos: Vec3::new(4.0, 70.0, -3.0),
            body_yaw: 0.0,
            head_yaw: 0.0,
            head_pitch: 0.0,
            anim_time: 0.0,
            walk_weight: 0.0,
            sneak_weight: 0.0,
            sleeping: false,
            seated: false,
            hurt: 0.0,
            skylight: 63,
            blocklight: petramond_world::light::BlockLight6::DARK,
            bones: Default::default(),
        }
    }

    fn swing_view(swing: f32) -> crate::HeldItemView {
        crate::HeldItemView {
            swing,
            swing_scale: 1.0,
            ..Default::default()
        }
    }

    fn bake(inst: &PlayerRenderInstance, swing: f32) -> Vec<ItemVertex> {
        let (mut v, mut i) = (Vec::new(), Vec::new());
        let (n, _, _) = build_player_body(
            player_model(),
            LightEnv::IDENTITY,
            inst,
            &[],
            &swing_view(swing),
            &crate::HeldItemView::default(),
            &mut v,
            &mut i,
        );
        assert_eq!(n as usize, i.len());
        v
    }

    fn hand(inst: &PlayerRenderInstance, swing: f32) -> Mat4 {
        let (mut v, mut i) = (Vec::new(), Vec::new());
        let (_, hand, _) = build_player_body(
            player_model(),
            LightEnv::IDENTITY,
            inst,
            &[],
            &swing_view(swing),
            &crate::HeldItemView::default(),
            &mut v,
            &mut i,
        );
        hand
    }

    fn off_hand(inst: &PlayerRenderInstance, off_swing: f32) -> Mat4 {
        let (mut v, mut i) = (Vec::new(), Vec::new());
        let (_, _, off) = build_player_body(
            player_model(),
            LightEnv::IDENTITY,
            inst,
            &[],
            &crate::HeldItemView::default(),
            &swing_view(off_swing),
            &mut v,
            &mut i,
        );
        off
    }

    #[test]
    fn body_bakes_and_walk_swings_layer_with_the_punch() {
        // Rest pose bakes geometry standing at the feet.
        let rest = bake(&instance(), 0.0);
        assert!(!rest.is_empty(), "player model bakes geometry");

        // Walking at two phases differs (limbs swing).
        let mut walking = instance();
        walking.walk_weight = 1.0;
        walking.anim_time = 0.0;
        let a = bake(&walking, 0.0);
        walking.anim_time = 0.25;
        let b = bake(&walking, 0.0);
        assert!(
            a.iter().zip(&b).any(|(x, y)| x.pos != y.pos),
            "walk animation moves the limbs"
        );

        // A mid-swing punch changes the pose ON TOP of the same walk phase.
        walking.anim_time = 0.25;
        let punched = bake(&walking, 0.4);
        assert!(
            b.iter().zip(&punched).any(|(x, y)| x.pos != y.pos),
            "the arm swing composes over the walk pose"
        );

        // Head-look moves geometry while idle (the head bone override is wired).
        let mut turned = instance();
        turned.head_yaw = 0.6;
        turned.head_pitch = 0.3;
        let looked = bake(&turned, 0.0);
        assert!(
            rest.iter().zip(&looked).any(|(x, y)| x.pos != y.pos),
            "head look poses the head"
        );
    }

    #[test]
    fn sneak_weight_poses_the_crouch_and_replaces_the_walk_cycle() {
        // Full sneak while standing still: a crouch stance, not the upright rest.
        let rest = bake(&instance(), 0.0);
        let mut crouched = instance();
        crouched.sneak_weight = 1.0;
        let stance = bake(&crouched, 0.0);
        assert!(
            rest.iter().zip(&stance).any(|(a, b)| a.pos != b.pos),
            "the sneak stance poses the body"
        );

        // A STILL sneaker holds the clip's first frame: the walk phase must not
        // leak into the stance.
        crouched.anim_time = 0.4;
        let stance_later = bake(&crouched, 0.0);
        assert!(
            stance
                .iter()
                .zip(&stance_later)
                .all(|(a, b)| a.pos == b.pos),
            "standing sneak freezes on the sneak clip's frame 0"
        );

        // A MOVING sneaker animates through the sneak clip (its own cycle)...
        crouched.walk_weight = 1.0;
        crouched.anim_time = 0.1;
        let step_a = bake(&crouched, 0.0);
        crouched.anim_time = 0.35;
        let step_b = bake(&crouched, 0.0);
        assert!(
            step_a.iter().zip(&step_b).any(|(a, b)| a.pos != b.pos),
            "sneak-walking advances the sneak cycle"
        );

        // ...and that cycle is the sneak clip, not the upright walk.
        let mut upright = instance();
        upright.walk_weight = 1.0;
        upright.anim_time = 0.1;
        let walking = bake(&upright, 0.0);
        assert!(
            walking.iter().zip(&step_a).any(|(a, b)| a.pos != b.pos),
            "sneak-walking is a different cycle than the upright walk"
        );
    }

    #[test]
    fn seated_swings_the_thighs_forward_and_hangs_the_shins() {
        // Seated (mounted): the height shrinks by roughly a thigh (the legs
        // fold), the lowest geometry rises off the anchor (no foot at y=0 —
        // the shins hang from the forward knees), and the knees stick out
        // toward the FACING (+Z at engine yaw 0), while the torso stays
        // upright (still much taller than a lying body).
        let standing = bake(&instance(), 0.0);
        let mut riding = instance();
        riding.seated = true;
        let seated = bake(&riding, 0.0);
        let span = |v: &[ItemVertex], axis: usize| {
            let lo = v.iter().map(|x| x.pos[axis]).fold(f32::MAX, f32::min);
            let hi = v.iter().map(|x| x.pos[axis]).fold(f32::MIN, f32::max);
            (lo, hi)
        };
        let (stand_lo, stand_hi) = span(&standing, 1);
        let (sit_lo, sit_hi) = span(&seated, 1);
        assert!(
            (stand_hi - stand_lo) - (sit_hi - sit_lo) > 0.25,
            "sitting folds the legs: {} vs {}",
            stand_hi - stand_lo,
            sit_hi - sit_lo
        );
        assert!(
            sit_lo > stand_lo + 0.2,
            "the shins hang above the anchor: {sit_lo} vs {stand_lo}"
        );
        assert!(
            sit_hi - sit_lo > 1.0,
            "the torso stays upright (not lying): {}",
            sit_hi - sit_lo
        );
        // Direction proof, not a reach pin: the folded legs must extend the
        // body's FACING side (+Z at yaw 0 — the head already reaches part of
        // the way there, so the margin is what the knees add past it).
        let (_, stand_z_hi) = span(&standing, 2);
        let (_, sit_z_hi) = span(&seated, 2);
        assert!(
            sit_z_hi > stand_z_hi + 0.05,
            "the knees extend toward the facing: {sit_z_hi} vs {stand_z_hi}"
        );
    }

    #[test]
    fn sleeping_lies_the_body_flat() {
        // Standing spans ~1.85 blocks of height; asleep the same model must lie
        // flat (height collapses to body thickness) and stretch horizontally.
        let standing = bake(&instance(), 0.0);
        let mut asleep = instance();
        asleep.sleeping = true;
        let lying = bake(&asleep, 0.0);
        let height = |v: &[ItemVertex]| {
            let ys: Vec<f32> = v.iter().map(|x| x.pos[1]).collect();
            ys.iter().fold(f32::MIN, |a, &b| a.max(b)) - ys.iter().fold(f32::MAX, |a, &b| a.min(b))
        };
        assert!(height(&standing) > 1.5, "standing body is tall");
        assert!(
            height(&lying) < 0.8,
            "sleeping body lies flat: {}",
            height(&lying)
        );
        // The body rests on the mattress plane: the torso (2 px half-thickness)
        // sits on it, and only the deeper head cube (4 px + hat inflate) may
        // nestle slightly below — into the pillow — never the whole body.
        let min_y = lying.iter().map(|v| v.pos[1]).fold(f32::MAX, f32::min);
        assert!(
            min_y >= asleep.pos.y - 0.2,
            "only a pillow-deep nestle below the mattress: {min_y}"
        );
    }

    #[test]
    fn walk_weight_blends_between_rest_and_the_full_cycle() {
        // A half-weight walk pose sits strictly between rest and the full cycle:
        // it differs from both, so stopping eases through intermediate poses
        // instead of flipping rest↔walk in one frame.
        let mut inst = instance();
        inst.anim_time = 0.25;
        inst.walk_weight = 0.0;
        let rest = bake(&inst, 0.0);
        inst.walk_weight = 1.0;
        let full = bake(&inst, 0.0);
        inst.walk_weight = 0.5;
        let half = bake(&inst, 0.0);
        assert!(
            rest.iter().zip(&half).any(|(a, b)| a.pos != b.pos),
            "half blend differs from rest"
        );
        assert!(
            full.iter().zip(&half).any(|(a, b)| a.pos != b.pos),
            "half blend differs from the full cycle"
        );
    }

    /// A HELD arm ignores the walk cycle; a COMPOSED one rides it.
    ///
    /// This is the whole difference between a nudge and a STANCE, and the
    /// failure is quiet: a guard raised with a composed offset still swings
    /// with the stride, because the swing is underneath it. `walk` and `sneak`
    /// both drive the shoulder AND the elbow, so this cannot be checked
    /// against a standing body — the animation has to be running.
    ///
    /// Measured RELATIVE TO THE TORSO, because the gaits also bob the root and
    /// a stance is not supposed to stop the body moving — only the arm.
    ///
    /// It also pins the part that surprises: holding a SHOULDER does not
    /// freeze the arm. Descendants keep their own animation relative to the
    /// held bone (which is what makes a held shoulder usable as a nudge
    /// point), so a stance has to hold every joint it owns.
    #[test]
    fn a_held_arm_ignores_the_walk_cycle_and_a_composed_one_rides_it() {
        let model = player_model();
        let body = model.bone_named("body").expect("torso");
        let shoulder = model.bone_named(HELD_SHOULDER_BONE).expect("main arm");
        let elbow = model.bone_named(HELD_ELBOW_BONE).expect("main forearm");
        let off_elbow = model.bone_named(OFF_ELBOW_BONE).expect("off forearm");
        let rot = Vec3::new(59.0, 19.0, -20.0);

        // The fist's pose in the TORSO's frame — what "the arm moved" means.
        let fist = |gait: &str, phase: f32, hold_shoulder: bool, hold_elbow: bool, bone: usize| {
            let anim = model.animation(gait).expect(gait);
            let mut pose = model.pose_layers(&[(anim, phase, 1.0)]);
            for (b, hold) in [(shoulder, hold_shoulder), (elbow, hold_elbow)] {
                if hold {
                    model.hold_bone(&mut pose, b, rot, Vec3::ZERO);
                } else {
                    model.apply_bone_offset(
                        &mut pose,
                        b,
                        petramond_world::bbmodel::euler_quat(rot),
                        Vec3::ZERO,
                    );
                }
            }
            pose[body].inverse() * pose[bone]
        };
        let moved = |a: Mat4, b: Mat4| {
            a.to_cols_array()
                .iter()
                .zip(b.to_cols_array())
                .map(|(x, y)| (x - y).abs())
                .fold(0.0f32, f32::max)
        };

        for gait in ["walk", "sneak"] {
            let rest = fist(gait, 0.0, true, true, elbow);
            for phase in [0.2, 0.45, 0.7] {
                assert!(
                    moved(rest, fist(gait, phase, true, true, elbow)) < 1e-4,
                    "a held arm must not move through the {gait} cycle"
                );
                // Non-vacuous: the OTHER arm swings at these very phases.
                assert!(
                    moved(
                        fist(gait, 0.0, true, true, off_elbow),
                        fist(gait, phase, true, true, off_elbow)
                    ) > 1e-3,
                    "the unheld arm must still swing, or this proves nothing"
                );
                assert!(
                    moved(rest, fist(gait, phase, false, false, elbow)) > 1e-3,
                    "a composed offset rides the {gait} instead of replacing it"
                );
                assert!(
                    moved(rest, fist(gait, phase, true, false, elbow)) > 1e-3,
                    "holding only the shoulder leaves the elbow animating"
                );
            }
        }
    }

    /// The third-person bbmodel attach adds `Rx(-90°)` and NOTHING ELSE.
    ///
    /// It carried an extra `Ry(180°)` until 2026-08-22, which turned every
    /// bbmodel item end-over-end in the fist relative to its own Blockbench
    /// preview — the game contradicting the model file. It hid for as long as
    /// it did because the only bbmodel items were the buckets, which are
    /// four-fold symmetric about exactly the axis it flipped; it surfaced the
    /// moment an item with a top and a bottom went in a hand.
    ///
    /// Pinned as DIRECTIONS rather than a matrix so it reads as the contract
    /// it is: display "up" (+Y) points forward out of the fist, display
    /// "forward" (+Z) points up, and neither the item's left nor its top is
    /// mirrored. The bucket is the subject because its authored third-person
    /// rotation is identity, so what is left IS the attach.
    #[test]
    fn the_third_person_attach_reorients_without_flipping_the_item() {
        use petramond_world::item::{ItemRenderKind, ItemType};

        let bucket = ItemType::by_name("petramond:wooden_bucket").expect("engine item");
        let ItemRenderKind::Model(kind) = bucket.render_kind() else {
            panic!("the bucket is a bbmodel item")
        };
        // In the ARM's own frame (the rest arm hangs unrotated, so its axes are
        // the authored model's: +Y up, −Z the body's front).
        let m = held_model_transform(Mat4::IDENTITY, kind);
        let dir = |v: Vec3| m.transform_vector3(v).normalize();

        let forward = dir(Vec3::Y);
        assert!(
            forward.z < -0.9,
            "display up must point out of the fist along the body's front, got {forward:?}"
        );
        let up = dir(Vec3::Z);
        assert!(
            up.y > 0.9,
            "display forward must point UP, not down — a down y is the old spurious yaw: {up:?}"
        );
        let right = dir(Vec3::X);
        assert!(
            right.x > 0.9,
            "the item's own +X must not be mirrored in the fist, got {right:?}"
        );
    }

    /// The load-bearing identity behind every off-hand pose in the engine:
    /// CONJUGATING a display transform by the x-flip IS
    /// `DisplayTransform::left_hand`.
    ///
    /// It is why neither view needs a per-hand rule for a claimed pose — the
    /// off-hand paths already conjugate the frame the pose rides in. Off-hand
    /// mirroring has been got wrong here before, and the failure is always
    /// silent: the item hangs somewhere plausible and wrong. Pin the algebra so
    /// a change of euler convention argues with a test, not with a playtest.
    #[test]
    fn conjugating_a_display_transform_is_exactly_the_left_hand_rule() {
        use petramond_world::block_model::DisplayTransform;

        for pose in [
            DisplayTransform {
                rotation: [0.0, 0.0, 0.0],
                translation: [1.5, -3.0, -5.0],
                ..Default::default()
            },
            DisplayTransform {
                rotation: [-16.0, 40.0, -25.0],
                translation: [0.0, 7.0, -2.0],
                ..Default::default()
            },
        ] {
            let conjugated = mirror_local(pose.base_matrix());
            let authored = pose.left_hand().base_matrix();
            let (a, b) = (conjugated.to_cols_array(), authored.to_cols_array());
            for (i, (x, y)) in a.iter().zip(&b).enumerate() {
                assert!(
                    (x - y).abs() < 1e-5,
                    "element {i} of {pose:?}: conjugated {x} vs left_hand {y}"
                );
            }
        }
    }

    /// A pose that changes nothing must leave the attach frame BIT-identical,
    /// not merely close: every hand without a mod pose takes this path every
    /// frame, and a matrix round trip there would move every held item in the
    /// game by a rounding error.
    #[test]
    fn an_identity_pose_leaves_the_hand_frame_untouched() {
        let inst = instance();
        let frame = hand(&inst, 0.0);
        assert_eq!(posed_hand(frame, &Default::default(), false), frame);
        assert_eq!(posed_hand(frame, &Default::default(), true), frame);
    }

    #[test]
    fn held_grip_is_on_the_visual_right_side() {
        let inst = instance();
        let grip = hand(&inst, 0.0).transform_point3(HAND_GRIP_PX);
        assert!(
            grip.x < inst.pos.x,
            "yaw 0 player-right is camera-right/world -X, grip at {grip:?}"
        );
    }

    /// Visual preview harness (NOT an assertion): the third-person body with
    /// the SAME item in both fists, seen from the front — so the off-hand
    /// attach transforms can be checked as the mirror of the right hand's.
    /// Rows: sprite item at rest, bbmodel item at rest, sprite mid off-jab.
    /// Run: `cargo test --lib -- --ignored --nocapture render_third_person_off_hand_preview`.
    /// Writes /tmp/third_person_off_hand.png.
    #[test]
    #[ignore = "visual preview harness; run explicitly to regenerate /tmp/third_person_off_hand.png"]
    fn render_third_person_off_hand_preview() {
        use crate::atlas::tile_uv;
        use crate::lighting::DynLight;
        use petramond_world::item::{ItemRenderKind, ItemType};

        let (w, h) = (640usize, 640usize);
        let rows = 3usize;
        let bg = [30u8, 32, 38];
        let mut color = vec![0u8; w * h * rows * 3];
        for px in color.chunks_mut(3) {
            px.copy_from_slice(&bg);
        }

        // Front camera: the body stands at the origin facing +Z (yaw 0), the
        // camera looks at its chest — their right hand is the viewer's left.
        let proj = Mat4::perspective_rh(55f32.to_radians(), w as f32 / h as f32, 0.05, 20.0);
        let view = Mat4::look_at_rh(
            Vec3::new(0.0, 1.25, 2.9),
            Vec3::new(0.0, 0.95, 0.0),
            Vec3::Y,
        );
        let mvp = proj * view;

        let model = player_model();
        let skin = (model.texture_rgba.as_slice(), model.tex_w, model.tex_h);
        let (model_atlas, maw, mah) = petramond_world::block_model::atlas().texture();

        // One z-buffered cell: raster `verts` (already world-space) with a
        // per-stream texture; both windings fill (the item streams draw on
        // the double-sided mob pipeline in game).
        let raster = |verts: &[ItemVertex],
                      tex: (&[u8], u32, u32),
                      row: usize,
                      zbuf: &mut [f32],
                      color: &mut [u8]| {
            let (pix, tw, th) = tex;
            for tri in verts.chunks_exact(3) {
                let mut s = [[0f32; 3]; 3];
                let mut ok = true;
                for (dst, v) in s.iter_mut().zip(tri) {
                    let c = mvp * glam::Vec4::new(v.pos[0], v.pos[1], v.pos[2], 1.0);
                    if c.w <= 1e-6 {
                        ok = false;
                        break;
                    }
                    let n = c / c.w;
                    *dst = [
                        (n.x * 0.5 + 0.5) * w as f32,
                        (1.0 - (n.y * 0.5 + 0.5)) * h as f32,
                        n.z,
                    ];
                }
                if !ok {
                    continue;
                }
                let (x0, y0, x1, y1, x2, y2) =
                    (s[0][0], s[0][1], s[1][0], s[1][1], s[2][0], s[2][1]);
                let area = (x1 - x0) * (y2 - y0) - (x2 - x0) * (y1 - y0);
                if area.abs() < 1e-6 {
                    continue;
                }
                let inv_area = 1.0 / area;
                let minx = x0.min(x1).min(x2).floor().max(0.0) as usize;
                let maxx = x0.max(x1).max(x2).ceil().min(w as f32 - 1.0) as usize;
                let miny = y0.min(y1).min(y2).floor().max(0.0) as usize;
                let maxy = y0.max(y1).max(y2).ceil().min(h as f32 - 1.0) as usize;
                for y in miny..=maxy {
                    for x in minx..=maxx {
                        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                        let w0 = ((x1 - px) * (y2 - py) - (x2 - px) * (y1 - py)) * inv_area;
                        let w1 = ((x2 - px) * (y0 - py) - (x0 - px) * (y2 - py)) * inv_area;
                        let w2 = 1.0 - w0 - w1;
                        if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                            continue;
                        }
                        let z = w0 * s[0][2] + w1 * s[1][2] + w2 * s[2][2];
                        let li = y * w + x;
                        if z >= zbuf[li] {
                            continue;
                        }
                        let u = w0 * tri[0].uv[0] + w1 * tri[1].uv[0] + w2 * tri[2].uv[0];
                        let v = w0 * tri[0].uv[1] + w1 * tri[1].uv[1] + w2 * tri[2].uv[1];
                        let tx = (u * tw as f32).clamp(0.0, tw as f32 - 1.0) as u32;
                        let ty = (v * th as f32).clamp(0.0, th as f32 - 1.0) as u32;
                        let ti = ((ty * tw + tx) * 4) as usize;
                        if pix[ti + 3] < 128 {
                            continue;
                        }
                        let shade = w0 * tri[0].shade + w1 * tri[1].shade + w2 * tri[2].shade;
                        zbuf[li] = z;
                        let o = ((row * h + y) * w + x) * 3;
                        color[o] = (pix[ti] as f32 * shade).min(255.0) as u8;
                        color[o + 1] = (pix[ti + 1] as f32 * shade).min(255.0) as u8;
                        color[o + 2] = (pix[ti + 2] as f32 * shade).min(255.0) as u8;
                    }
                }
            }
        };

        // The pickaxe texture, re-addressed through its atlas rect so the
        // extruded verts' atlas UVs sample the source PNG.
        let pick_src = format!(
            "{}/../../assets/textures/stone_pickaxe.png",
            env!("CARGO_MANIFEST_DIR")
        );
        let pick_img = image::open(&pick_src).expect("texture").to_rgba8();
        let (ptw, pth) = pick_img.dimensions();
        let pick_tile = match ItemType::StonePickaxe.render_kind() {
            ItemRenderKind::Sprite(t) => t,
            _ => panic!("pickaxe is a sprite"),
        };
        let [au0, av0, au1, av1] = tile_uv(pick_tile);
        let sprite_stream = |m: Mat4| -> Vec<ItemVertex> {
            let mut v = Vec::new();
            crate::item_model::build_extruded_item_lit(
                pick_tile,
                DynLight::FULL,
                LightEnv::IDENTITY,
                &mut v,
            );
            transform_positions(v.iter_mut().map(|x| &mut x.pos), m);
            // Re-normalize the atlas rect onto the source PNG for sampling.
            for x in &mut v {
                x.uv[0] = (x.uv[0] - au0) / (au1 - au0);
                x.uv[1] = (x.uv[1] - av0) / (av1 - av0);
            }
            v
        };
        let bucket_kind = match ItemType::WoodenBucket.render_kind() {
            ItemRenderKind::Model(k) => k,
            _ => panic!("bucket is a model item"),
        };
        let model_stream = |m: Mat4| -> Vec<ItemVertex> {
            let (mut tv, mut ti) = (Vec::new(), Vec::new());
            crate::item_model::build_block_model_item(
                bucket_kind,
                m,
                DynLight::FULL,
                LightEnv::IDENTITY,
                None,
                &mut tv,
                &mut ti,
            );
            let mut flat = Vec::with_capacity(ti.len());
            for &i in &ti {
                flat.push(tv[i as usize]);
            }
            flat
        };

        let (mut bv, mut bi) = (Vec::new(), Vec::new());
        // The shared fixture parks the body away from the origin; the camera
        // above looks at the origin, so stand the body there.
        let mut inst = instance();
        inst.pos = Vec3::ZERO;
        for (row, (off_swing, model_row)) in
            [(0.0, false), (0.0, true), (0.5, false)].iter().enumerate()
        {
            let held_view = swing_view(0.0);
            let off_view = swing_view(*off_swing);
            let (_, hand, off_hand) = build_player_body(
                model,
                LightEnv::IDENTITY,
                &inst,
                &[],
                &held_view,
                &off_view,
                &mut bv,
                &mut bi,
            );
            let mut zbuf = vec![f32::INFINITY; w * h];
            // Body (indexed → flat triangles), skin texture.
            let mut body = Vec::with_capacity(bi.len());
            for &i in &bi {
                body.push(bv[i as usize]);
            }
            raster(&body, skin, row, &mut zbuf, &mut color);
            if *model_row {
                raster(
                    &model_stream(held_model_transform(hand, bucket_kind)),
                    (model_atlas, maw, mah),
                    row,
                    &mut zbuf,
                    &mut color,
                );
                raster(
                    &model_stream(held_model_transform_off(off_hand, bucket_kind)),
                    (model_atlas, maw, mah),
                    row,
                    &mut zbuf,
                    &mut color,
                );
            } else {
                raster(
                    &sprite_stream(held_sprite_transform(hand)),
                    (pick_img.as_raw(), ptw, pth),
                    row,
                    &mut zbuf,
                    &mut color,
                );
                raster(
                    &sprite_stream(held_sprite_transform_off(off_hand)),
                    (pick_img.as_raw(), ptw, pth),
                    row,
                    &mut zbuf,
                    &mut color,
                );
            }
            println!(
                "row {row}: {} (off_swing {off_swing})",
                if *model_row {
                    "bucket both hands"
                } else {
                    "pickaxe both hands"
                }
            );
        }
        image::save_buffer(
            "/tmp/third_person_off_hand.png",
            &color,
            w as u32,
            (h * rows) as u32,
            image::ColorType::Rgb8,
        )
        .expect("save png");
        println!("wrote /tmp/third_person_off_hand.png (front view; their right = your left)");
    }

    #[test]
    fn off_hand_grip_is_on_the_visual_left_side_and_jabs_inward() {
        let inst = instance();
        // The off grip point mirrors the main one in the arm-local frame.
        let grip_local = Vec3::new(-HAND_GRIP_PX.x, HAND_GRIP_PX.y, HAND_GRIP_PX.z);
        let rest = off_hand(&inst, 0.0).transform_point3(grip_local);
        assert!(
            rest.x > inst.pos.x,
            "yaw 0 player-left is world +X, off grip at {rest:?}"
        );
        for swing in [0.1, 0.25, 0.5, 0.75] {
            let grip = off_hand(&inst, swing).transform_point3(grip_local);
            assert!(
                grip.x < rest.x,
                "the left-hand jab punches inward at {swing}: {grip:?} vs {rest:?}"
            );
            assert!(
                grip.z > rest.z,
                "the left-hand jab still punches forward at {swing}: {grip:?} vs {rest:?}"
            );
        }
    }

    #[test]
    fn held_swing_moves_visual_right_hand_toward_center() {
        let inst = instance();
        let rest = hand(&inst, 0.0).transform_point3(HAND_GRIP_PX);
        for swing in [0.1, 0.25, 0.4, 0.5, 0.75, 0.9] {
            let grip = hand(&inst, swing).transform_point3(HAND_GRIP_PX);
            assert!(
                grip.x > rest.x,
                "visual right-hand swing should punch inward, not hook farther right at {swing}: {grip:?} vs {rest:?}"
            );
            assert!(
                grip.z > rest.z,
                "visual right-hand swing should still punch forward at {swing}: {grip:?} vs {rest:?}"
            );
        }

        let done = hand(&inst, 1.0).transform_point3(HAND_GRIP_PX);
        assert!(
            (done - rest).length() < 0.001,
            "swing phase 1.0 should return to rest: {done:?} vs {rest:?}"
        );
    }
}
