use glam::{Mat4, Vec3};

use super::lighting::DynLight;
use super::skinned::{bone_slots, SkinBatch, SkinLook};
use super::PlayerRenderInstance;
use petramond::player::model::PLAYER_MODEL_SCALE;
use petramond::player::rigs::Rig;
use petramond_world::bbmodel::Model;

/// The grip point in model pixels, in the main grip's rest frame: centred in
/// the fist (the lower arm spans x 4..8, ends at y 12), a touch toward the
/// front. The authored model is rotated by π to face engine-forward, which
/// makes the authored left arm the visual right hand — hence the shipped
/// row's main grip on the left arm. The off grip's attach transforms are the
/// main hand's conjugated by an arm-local X mirror ([`mirror_local`]), so the
/// two fists stay symmetric by construction.
const HAND_GRIP_PX: Vec3 = Vec3::new(6.0, 11.0, -1.5);

fn mirror_local(m: Mat4) -> Mat4 {
    let s = Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0));
    s * m * s
}

fn reflect_local(m: Mat4) -> Mat4 {
    Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)) * m
}

const SPRITE_WORLD_SIZE: f32 = 0.60;
const BLOCK_WORLD_SIZE: f32 = 0.30;

pub(super) fn place_player_body(
    rig: &Rig,
    inst: &PlayerRenderInstance,
    pose: &[Mat4],
    render_origin: glam::IVec3,
    batch: &mut SkinBatch,
) -> (Mat4, Mat4) {
    let global = Mat4::from_translation(inst.pos.relative_to(render_origin)) * inst.placement;
    push_body(&rig.model, rig.grips, pose, global, inst, batch)
}

fn push_body(
    model: &Model,
    grips: [usize; 2],
    pose: &[Mat4],
    global: Mat4,
    inst: &PlayerRenderInstance,
    batch: &mut SkinBatch,
) -> (Mat4, Mat4) {
    batch.push(
        pose,
        global,
        bone_slots(model),
        SkinLook {
            hurt: inst.hurt,
            emitter_tint: inst.emitter_tint,
            emitter_self_lit: inst.emitter_self_lit,
            light: DynLight::new(inst.skylight, inst.blocklight),
            hidden: 0,
        },
    );

    let [hand, off_hand] =
        grips.map(|bone| global * pose.get(bone).copied().unwrap_or(Mat4::IDENTITY));
    (hand, off_hand)
}

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

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Grip {
    pub frame: Mat4,
    pub point: Vec3,
    pub px: f32,
    pub roll: f32,
}

impl Grip {
    pub(super) fn body(frame: Mat4) -> Self {
        Self {
            frame,
            point: HAND_GRIP_PX,
            px: PLAYER_MODEL_SCALE,
            roll: 0.0,
        }
    }

    pub(super) fn body_off(frame: Mat4) -> Self {
        Self {
            point: HAND_GRIP_PX * Vec3::new(-1.0, 1.0, 1.0),
            ..Self::body(frame)
        }
    }

    fn mirrored(self) -> Self {
        Self {
            point: self.point * Vec3::new(-1.0, 1.0, 1.0),
            ..self
        }
    }
}

/// Seat an extruded sprite at its authored grip, or use the shared diagonal tool hold.
pub(super) fn held_sprite_at(
    grip: Grip,
    pose: Option<petramond_world::item::SpriteHeldPose>,
) -> Mat4 {
    grip.frame * sprite_hold(grip, pose)
}

fn sprite_hold(grip: Grip, pose: Option<petramond_world::item::SpriteHeldPose>) -> Mat4 {
    let size = SPRITE_WORLD_SIZE / grip.px;
    if let Some(pose) = pose {
        return Mat4::from_translation(grip.point)
            * Mat4::from_rotation_y(pose.yaw)
            * Mat4::from_rotation_x(pose.pitch)
            * Mat4::from_rotation_z(pose.roll)
            * Mat4::from_scale(Vec3::splat(size * pose.scale))
            * Mat4::from_translation(-Vec3::from(pose.grip));
    }
    let rot = Mat4::from_rotation_x(-65f32.to_radians())
        * Mat4::from_rotation_y(-std::f32::consts::FRAC_PI_2)
        * Mat4::from_rotation_z(55f32.to_radians())
        * Mat4::from_axis_angle(
            Vec3::new(
                std::f32::consts::FRAC_1_SQRT_2,
                std::f32::consts::FRAC_1_SQRT_2,
                0.0,
            ),
            grip.roll,
        );
    let axis = rot.transform_vector3(Vec3::new(
        std::f32::consts::FRAC_1_SQRT_2,
        std::f32::consts::FRAC_1_SQRT_2,
        0.0,
    ));
    Mat4::from_translation(grip.point + axis * (0.30 * size))
        * rot
        * Mat4::from_scale(Vec3::splat(size))
}

pub(super) fn held_block_at(grip: Grip) -> Mat4 {
    grip.frame * block_hold(grip)
}

fn block_hold(grip: Grip) -> Mat4 {
    Mat4::from_translation(grip.point + Vec3::new(0.0, -0.5, -2.0))
        * Mat4::from_rotation_y(std::f32::consts::FRAC_PI_4)
        * Mat4::from_scale(Vec3::splat(BLOCK_WORLD_SIZE / grip.px))
}

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
pub(super) fn held_sprite_off_at(
    grip: Grip,
    pose: Option<petramond_world::item::SpriteHeldPose>,
) -> Mat4 {
    grip.frame * reflect_local(sprite_hold(grip.mirrored(), pose))
}

pub(super) fn held_block_off_at(grip: Grip) -> Mat4 {
    grip.frame * mirror_local(block_hold(grip.mirrored()))
}

/// Off-hand bbmodel attach, third-person twin of `hand::held_model_off`.
/// Frame mirror is just the grip's x negated, since x-rotations are mirror-symmetric. Pose is
/// [`DisplayTransform::left_hand`]. Geometry and its `display_from_unit` rebase aren't reflected.
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

pub(super) fn transform_positions<'a>(pos: impl Iterator<Item = &'a mut [f32; 3]>, m: Mat4) {
    for p in pos {
        *p = m.transform_point3(Vec3::from(*p)).to_array();
    }
}

#[cfg(test)]
mod tests;
