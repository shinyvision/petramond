//! Third-person player body: the rigs catalog's body rig, posed by the
//! client's animation, placed each frame into the GPU skinning batch
//! (`crate::skinned`) — one bone palette run and one instance row per body,
//! skinned in the vertex shader from the rig's static mesh and drawn in the
//! mob pass with the player's own skin texture bound. The items attach at
//! the row's grip bones.
//!
//! The pose itself — locomotion, the body animator, sleep and seat, head-look
//! and claimed bone offsets — is the client's (`animation::pose`); the
//! renderer consumes the finished bone array and placement.

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

/// Place one posed player body of `rig` into `batch` (one palette run + one
/// instance row, skinned from the rig's static mesh), answering the visual
/// right- and left-hand attach frames (model-pixel space under the placed,
/// scaled body) for the held items. `pose` is the body's final model-space
/// bones, posed by the client's animation; the renderer only stands them at
/// the feet under the body's placement.
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

/// Push the posed model under `global` — lit and hurt-tinted per instance —
/// and return the `[main, off]` grip bones' world transforms.
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

    // A clip turning a grip bone turns the item in the fist.
    let [hand, off_hand] =
        grips.map(|bone| global * pose.get(bone).copied().unwrap_or(Mat4::IDENTITY));
    (hand, off_hand)
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
    /// A held sprite's roll about its own length (radians); the player's
    /// seats are authored at none.
    pub roll: f32,
}

impl Grip {
    /// The body's main-hand grip on an arm frame from [`place_player_body`].
    pub(super) fn body(frame: Mat4) -> Self {
        Self {
            frame,
            point: HAND_GRIP_PX,
            px: PLAYER_MODEL_SCALE,
            roll: 0.0,
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
        * Mat4::from_rotation_z(55f32.to_radians())
        * Mat4::from_axis_angle(
            Vec3::new(
                std::f32::consts::FRAC_1_SQRT_2,
                std::f32::consts::FRAC_1_SQRT_2,
                0.0,
            ),
            grip.roll,
        );
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
/// [`petramond_mesh::Vertex`] and explicit-UV
/// [`ItemVertex`](crate::item_model::ItemVertex)) share it.
pub(super) fn transform_positions<'a>(pos: impl Iterator<Item = &'a mut [f32; 3]>, m: Mat4) {
    for p in pos {
        *p = m.transform_point3(Vec3::from(*p)).to_array();
    }
}

#[cfg(test)]
mod tests;
