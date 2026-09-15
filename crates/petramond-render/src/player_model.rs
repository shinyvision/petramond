//! Third-person player body: the rigs catalog's body rig posed and baked each
//! frame into the mob-layout `ItemVertex` stream (world space, drawn in the mob
//! pass with the player's own skin texture bound).
//!
//! Pose composition, in order: the locomotion table's clips blended by their
//! weights (`locomotion.rs`) as the GROUND, the body animator's graph
//! evaluated over it (actions override and add to the walk), the sleep and
//! seat poses, the head-look override on the `head` bone (compensated for
//! the yaw the animator added to the row's `twist` bones, so the gaze stays
//! put), then the claimed bone offsets. The items attach at the row's grip
//! bones.
//!
//! The model is authored front = −Z (the skin's face texture sits on the north
//! face), while engine yaw 0 faces +Z, so the body renders with `yaw + π`.

pub(crate) mod body_animator;
mod locomotion;

pub(crate) use body_animator::{BodyAnimator, BodyAnimators, LOCAL_BODY};

use glam::{Mat4, Quat, Vec3};

use super::item_model::ItemVertex;
use super::lighting::{DynLight, LightEnv};
use super::mob_model::{bake_model_cubes, body_tint};
use super::PlayerRenderInstance;
use petramond::player::model::{PLAYER_HIP_HEIGHT, PLAYER_MODEL_SCALE};
use petramond::player::rigs::Rig;
use petramond_world::animation::LocalPose;
use petramond_world::bbmodel::Model;

/// The grip point in model pixels, in the main grip's rest frame: centred in
/// the fist (the lower arm spans x 4..8, ends at y 12), a touch toward the
/// front. The authored model is rotated by π to face engine-forward, which
/// makes the authored left arm the visual right hand — hence the shipped
/// row's main grip on the left arm. The off grip's attach transforms are the
/// main hand's conjugated by an arm-local X mirror ([`mirror_local`]), so the
/// two fists stay symmetric by construction.
const HAND_GRIP_PX: Vec3 = Vec3::new(6.0, 11.0, -1.5);

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

/// What drives a body's animator this frame.
pub(super) struct BodyDrive<'a> {
    pub animator: &'a mut BodyAnimator,
    pub frames: Option<&'a [crate::HeldItemFrame; 2]>,
    pub inputs: crate::AnimatorInputs<'a>,
    pub dt: f32,
}

/// Pose and bake one player body of `rig` into `verts`/`indices`, answering
/// its index count and the visual right- and left-hand attach frames
/// (model-pixel space under the placed, scaled body) for the held items. With
/// a `drive` the locomotion pose is the animator's ground and the animator's
/// pose is what bakes; without one (no body graph) the locomotion pose bakes
/// as it is.
#[allow(clippy::too_many_arguments)]
pub(super) fn build_player_body(
    rig: &Rig,
    env: LightEnv,
    inst: &PlayerRenderInstance,
    render_origin: glam::IVec3,
    bones: &[crate::BoneOffset],
    drive: Option<BodyDrive<'_>>,
    verts: &mut Vec<ItemVertex>,
    indices: &mut Vec<u32>,
) -> (u32, Mat4, Mat4) {
    let model = &rig.model;
    let pos = inst.pos.relative_to(render_origin);
    verts.clear();
    indices.clear();

    let layers = locomotion::layers(model, inst);
    let mut twist_total = 0.0;
    let mut pose = match drive {
        Some(drive) => {
            let mut ground = LocalPose::rest(model.bones().len());
            for (anim, time, weight) in &layers {
                ground.add_clip(anim, *time, weight.clamp(0.0, 1.0));
            }
            drive
                .animator
                .update(inst, drive.frames, drive.inputs, drive.dt, &ground);
            let local = drive.animator.pose();
            // The torso twist the animator added: head-look takes it back out
            // so the gaze holds while the body swings.
            for &bone in &rig.twist {
                twist_total += (local.rotation(bone).y - ground.rotation(bone).y).to_radians();
            }
            local.resolve(model)
        }
        None if layers.is_empty() => model.rest_pose(),
        None => model.pose_layers(&layers),
    };
    let head_animated = |hb: usize| layers.iter().any(|(a, _, _)| a.affects_bone(hb));

    // Asleep: the rest pose lying on its back — rotated flat about the feet,
    // head toward `body_yaw`, floated onto the mattress. Head-look and the arm
    // swing rest with it.
    if inst.sleeping {
        let global = Mat4::from_translation(pos + Vec3::new(0.0, LIE_LIFT, 0.0))
            * Mat4::from_rotation_y(inst.body_yaw)
            * Mat4::from_rotation_x(std::f32::consts::FRAC_PI_2)
            * Mat4::from_scale(Vec3::splat(PLAYER_MODEL_SCALE));
        return bake_cubes(model, rig.grips, &pose, global, inst, env, verts, indices);
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
    // Claimed bone offsets LAST, so they compose on top of every engine layer
    // (walk, sneak, head-look, the animator's actions) rather than
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
    let global = Mat4::from_translation(pos)
        * Mat4::from_rotation_y(inst.body_yaw + std::f32::consts::PI)
        * lean
        * Mat4::from_scale(Vec3::splat(PLAYER_MODEL_SCALE));
    bake_cubes(model, rig.grips, &pose, global, inst, env, verts, indices)
}

/// Emit every cube of the posed model under `global`, lit and hurt-tinted, and
/// return the index count plus the `[main, off]` grip bones' world transforms.
#[allow(clippy::too_many_arguments)]
fn bake_cubes(
    model: &Model,
    grips: [usize; 2],
    pose: &[Mat4],
    global: Mat4,
    inst: &PlayerRenderInstance,
    env: LightEnv,
    verts: &mut Vec<ItemVertex>,
    indices: &mut Vec<u32>,
) -> (u32, Mat4, Mat4) {
    let tint = body_tint(
        inst.hurt,
        inst.emitter_tint,
        DynLight::new(inst.skylight, inst.blocklight),
        env,
        inst.emitter_self_lit,
    );
    bake_model_cubes(model, pose, global, tint, |_| false, verts, indices);

    // A clip turning a grip bone turns the item in the fist.
    let [hand, off_hand] =
        grips.map(|bone| global * pose.get(bone).copied().unwrap_or(Mat4::IDENTITY));
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

/// Where a hand holds its item: the posed arm frame (everything placing the
/// rig already folded in), the grip point in that frame's rig pixels, and one
/// rig pixel's size in the frame's output units. The body and the
/// first-person rig seat every held render kind through the same transforms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Grip {
    pub frame: Mat4,
    pub point: Vec3,
    pub px: f32,
}

impl Grip {
    /// The body's main-hand grip on an arm frame from [`build_player_body`].
    pub(super) fn body(frame: Mat4) -> Self {
        Self {
            frame,
            point: HAND_GRIP_PX,
            px: PLAYER_MODEL_SCALE,
        }
    }

    /// The body's off-hand grip: the main grip mirrored onto the other arm.
    pub(super) fn body_off(frame: Mat4) -> Self {
        Self {
            point: HAND_GRIP_PX * Vec3::new(-1.0, 1.0, 1.0),
            ..Self::body(frame)
        }
    }

    /// This grip with its point reflected across the frame's YZ plane: the
    /// off-hand twins compose the MAIN hand's hold and mirror it whole.
    fn mirrored(self) -> Self {
        Self {
            point: self.point * Vec3::new(-1.0, 1.0, 1.0),
            ..self
        }
    }
}

/// World transform for the EXTRUDED sprite item (unit XY slab). Tool art runs
/// diagonally (handle lower-left, head upper-right); rolling the art 55° in its
/// plane stands the tool along the sprite's +Y, the yaw turns the slab edge-on
/// (flat face to the sides), and the X tilt lays the tool axis pointing FORWARD
/// out of the fist with a slight rise. The sprite centre is then shifted along
/// that axis so the fist grips the HANDLE end, not the middle/head.
pub(super) fn held_sprite_at(grip: Grip) -> Mat4 {
    grip.frame * sprite_hold(grip)
}

fn sprite_hold(grip: Grip) -> Mat4 {
    let size = SPRITE_WORLD_SIZE / grip.px;
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
    Mat4::from_translation(grip.point + axis * (0.30 * size))
        * rot
        * Mat4::from_scale(Vec3::splat(size))
}

/// World transform for a held block mini-cube (built origin-centred, unit size):
/// a corner turned toward the front, floated just ahead of the fist.
pub(super) fn held_block_at(grip: Grip) -> Mat4 {
    grip.frame * block_hold(grip)
}

fn block_hold(grip: Grip) -> Mat4 {
    Mat4::from_translation(grip.point + Vec3::new(0.0, -0.5, -2.0))
        * Mat4::from_rotation_y(std::f32::consts::FRAC_PI_4)
        * Mat4::from_scale(Vec3::splat(BLOCK_WORLD_SIZE / grip.px))
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
pub(super) fn held_model_at(
    grip: Grip,
    kind: petramond_world::block_model::BlockModelKind,
) -> Mat4 {
    let pose = &petramond_world::block_model::display(kind).thirdperson_righthand;
    grip.frame
        * model_frame(grip)
        * pose.base_matrix()
        * petramond_world::block_model::instance(kind).display_from_unit
}

/// The hand-layer frame a display pose composes in: the grip point, display
/// blocks in rig pixels, display up pointing forward out of the fist.
fn model_frame(grip: Grip) -> Mat4 {
    Mat4::from_translation(grip.point)
        * Mat4::from_scale(Vec3::splat(1.0 / grip.px))
        * Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2)
}

/// OFF-hand (left fist) twins of the three attach transforms. The sprite and
/// bbmodel twins are the right hand's composition truly REFLECTED
/// ([`reflect_local`]) — the left fist holds the mirror image, so a tool's
/// head still points forward and a model shows the same face its Blockbench
/// lefthand preview shows. The block mini-cube keeps the winding-preserving
/// conjugation (its pipeline culls, and a cube's three-quarter view survives
/// conjugation).
pub(super) fn held_sprite_off_at(grip: Grip) -> Mat4 {
    grip.frame * reflect_local(sprite_hold(grip.mirrored()))
}

pub(super) fn held_block_off_at(grip: Grip) -> Mat4 {
    grip.frame * mirror_local(block_hold(grip.mirrored()))
}

/// The bbmodel off-hand attach — the third-person twin of the first-person
/// rule (`hand::held_model_off`, Blockbench's lefthand composition): the
/// hand-layer FRAME mirrors by conjugation (`mirror_local` — for this frame
/// that is just the grip's x negated: an x-rotation is mirror-symmetric), the
/// pose is the slot's values with `translation.x` /
/// `rotation.y` / `rotation.z` negated ([`DisplayTransform::left_hand`],
/// authored `thirdperson_lefthand` included), and the geometry + its
/// `display_from_unit` rebase stay untouched — no reflection.
pub(super) fn held_model_off_at(
    grip: Grip,
    kind: petramond_world::block_model::BlockModelKind,
) -> Mat4 {
    let display = petramond_world::block_model::display(kind);
    let pose = display
        .thirdperson_lefthand
        .as_ref()
        .unwrap_or(&display.thirdperson_righthand)
        .left_hand();
    grip.frame
        * mirror_local(model_frame(grip.mirrored()))
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
