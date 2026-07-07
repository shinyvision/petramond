//! Third-person player body: the compiled `player.bbmodel` posed and baked each
//! frame into the mob-layout `ItemVertex` stream (world space, drawn in the mob
//! pass with the player's own skin texture bound).
//!
//! Pose composition, in order: the authored `walk` animation blended by
//! `walk_weight` (so starts/stops ease instead of snapping — [`Model::pose_scaled`]),
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
use super::lighting::{light_rgb, DynLight, LightEnv};
use super::mob_model::{hurt_tint, push_face};
use super::PlayerRenderInstance;
use crate::bbmodel::{euler_quat, Model};
use crate::mesh::face::Face;
use crate::player::model::PLAYER_MODEL_SCALE;

/// The grip point in model pixels, in the visual-right arm's rest frame: centred
/// in the fist (the lower arm spans x 4..8, ends at y 12), a touch toward the
/// front. The authored model is rotated by π to face engine-forward; under this
/// engine's camera convention that makes the authored left arm the visual right
/// hand in third person.
const HAND_GRIP_PX: Vec3 = Vec3::new(6.0, 11.0, -1.5);
const HELD_SHOULDER_BONE: &str = "left_shoulder";
const HELD_ELBOW_BONE: &str = "left_elbow";

/// World-space size (blocks) of the held sprite-item slab.
const SPRITE_WORLD_SIZE: f32 = 0.60;
/// World-space size (blocks) of the held block mini-cube.
const BLOCK_WORLD_SIZE: f32 = 0.30;

/// How far the lying (sleeping) body's anchor floats above the mattress top:
/// half the 4 px body thickness plus a hair of clearance over the bed model.
const LIE_LIFT: f32 = 2.2 * PLAYER_MODEL_SCALE;

/// Bake the player body posed for this frame. Returns the emitted index count
/// plus the visual right-hand world transform (model-pixel units under the
/// placed, scaled body) for attaching the held item.
pub(super) fn build_player_body(
    model: &Model,
    env: LightEnv,
    inst: &PlayerRenderInstance,
    swing: f32,
    swing_scale: f32,
    eat: f32,
    eat_bob: f32,
    verts: &mut Vec<ItemVertex>,
    indices: &mut Vec<u32>,
) -> (u32, Mat4) {
    verts.clear();
    indices.clear();

    let walk = model.animation("walk");
    let anim = (inst.walk_weight > 0.001 && !inst.sleeping)
        .then_some(walk)
        .flatten();
    let mut pose = match anim {
        Some(a) => model.pose_scaled(a, inst.anim_time, inst.walk_weight),
        None => model.rest_pose(),
    };

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
    if twist != 0.0 {
        if let Some(body) = model.bone_named("body") {
            model.apply_bone_rotation(&mut pose, body, Quat::from_rotation_y(twist));
        }
    }
    if let Some(hb) = model.head_bone() {
        if !anim.is_some_and(|a| a.affects_bone(hb)) {
            model.apply_head_look(&mut pose, hb, inst.head_yaw - twist, inst.head_pitch);
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
    // Eating: hold the forearm up so the food sits at the mouth (following the
    // gaze pitch like the swing does), bobbing with each bite. Blended by the
    // shared `eat` channel, so start/finish/abort ease exactly like first person.
    if eat > 0.0 {
        if let Some(shoulder) = model.bone_named(HELD_SHOULDER_BONE) {
            let raise = 1.35 + (inst.head_pitch + 0.7) * 0.35;
            let rot = Quat::from_rotation_x(eat * (raise + eat_bob * 0.04));
            model.apply_bone_rotation(&mut pose, shoulder, rot);
        }
    }

    // Authored front is −Z; engine yaw 0 faces +Z — hence the π.
    let global = Mat4::from_translation(inst.pos)
        * Mat4::from_rotation_y(inst.body_yaw + std::f32::consts::PI)
        * Mat4::from_scale(Vec3::splat(PLAYER_MODEL_SCALE));
    bake_cubes(model, &pose, global, inst, env, verts, indices)
}

/// Emit every cube of the posed model under `global`, lit and hurt-tinted, and
/// return the index count plus the visual right-hand world transform.
fn bake_cubes(
    model: &Model,
    pose: &[Mat4],
    global: Mat4,
    inst: &PlayerRenderInstance,
    env: LightEnv,
    verts: &mut Vec<ItemVertex>,
    indices: &mut Vec<u32>,
) -> (u32, Mat4) {
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

    let hand_bone = model
        .bone_named(HELD_ELBOW_BONE)
        .or_else(|| model.bone_named(HELD_SHOULDER_BONE));
    let hand = global
        * hand_bone
            .and_then(|b| pose.get(b).copied())
            .unwrap_or(Mat4::IDENTITY);
    (indices.len() as u32, hand)
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
pub(super) fn held_model_transform(hand: Mat4, kind: crate::block_model::BlockModelKind) -> Mat4 {
    let pose = &crate::block_model::display(kind).thirdperson_righthand;
    hand * Mat4::from_translation(HAND_GRIP_PX)
        * Mat4::from_scale(Vec3::splat(1.0 / PLAYER_MODEL_SCALE))
        * Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2)
        * Mat4::from_rotation_y(std::f32::consts::PI)
        * pose.base_matrix()
        * crate::block_model::instance(kind).display_from_unit
}

/// CPU-transform the vertex positions appended at `start..` by `m` — the packed
/// block-vertex counterpart of baking in model space then placing in the world
/// (the opaque pipeline has no per-draw model matrix).
pub(super) fn transform_positions(verts: &mut [crate::mesh::Vertex], start: usize, m: Mat4) {
    for v in verts[start..].iter_mut() {
        let p = m.transform_point3(Vec3::from(v.pos));
        v.pos = p.to_array();
    }
}

/// As [`transform_positions`] but for the explicit-UV [`ItemVertex`] stream.
pub(super) fn transform_item_positions(verts: &mut [ItemVertex], start: usize, m: Mat4) {
    for v in verts[start..].iter_mut() {
        let p = m.transform_point3(Vec3::from(v.pos));
        v.pos = p.to_array();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::model::player_model;

    fn instance() -> PlayerRenderInstance {
        PlayerRenderInstance {
            pos: Vec3::new(4.0, 70.0, -3.0),
            body_yaw: 0.0,
            head_yaw: 0.0,
            head_pitch: 0.0,
            anim_time: 0.0,
            walk_weight: 0.0,
            sleeping: false,
            hurt: 0.0,
            skylight: 63,
            blocklight: 0,
        }
    }

    fn bake(inst: &PlayerRenderInstance, swing: f32) -> Vec<ItemVertex> {
        let (mut v, mut i) = (Vec::new(), Vec::new());
        let (n, _) = build_player_body(
            player_model(),
            LightEnv::IDENTITY,
            inst,
            swing,
            1.0,
            0.0,
            0.0,
            &mut v,
            &mut i,
        );
        assert_eq!(n as usize, i.len());
        v
    }

    fn hand(inst: &PlayerRenderInstance, swing: f32) -> Mat4 {
        let (mut v, mut i) = (Vec::new(), Vec::new());
        let (_, hand) = build_player_body(
            player_model(),
            LightEnv::IDENTITY,
            inst,
            swing,
            1.0,
            0.0,
            0.0,
            &mut v,
            &mut i,
        );
        hand
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

    #[test]
    fn held_grip_is_on_the_visual_right_side() {
        let inst = instance();
        let grip = hand(&inst, 0.0).transform_point3(HAND_GRIP_PX);
        assert!(
            grip.x < inst.pos.x,
            "yaw 0 player-right is camera-right/world -X, grip at {grip:?}"
        );
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
