//! First-person held-item / hand geometry.
//!
//! Builds, each frame, the small full-bright model shown in the lower-right of the
//! screen from the flat [`HeldItemView`] prepared by the renderer:
//! - `item == None` -> a skin-colored first-person ARM cuboid
//!   ([`block_model::cube_solid`]) rising from the lower-right toward centre,
//!   tilted up, broad back-of-hand face to the camera + a darker side visible.
//! - `item` is a block-cube -> the [`block_model::cube_textured`] block, held with a
//!   corner toward the camera (MC-style three-quarter view).
//! - `item` is a sprite (flower / future tool) -> NOT model3d geometry; the
//!   renderer instead draws an EXTRUDED 3D item (see [`super::item_model`]) via
//!   the dedicated `item3d` pipeline at the held three-quarter angle reported by
//!   [`held_sprite`].
//!
//! The hand is drawn over the world (no depth attachment), so it uses its OWN
//! fixed first-person perspective rather than the world camera — the returned MVP
//! is a complete clip-space transform. The punch (`swing` 0..1 sawtooth while
//! mining, one-shot for a break/place) and its `swing_scale` amplitude (softer
//! for a place than a mining hit) are folded into that transform here.

use glam::{Mat4, Quat, Vec3};

use super::item_cube::{push_block_item_cube_lit_with_state, push_cube_solid_lit};
use super::lighting::DynLight;
use super::HeldItemView;
use crate::atlas::Tile;
use crate::block::Block;
use crate::item::ItemRenderKind;
use crate::mesh::Vertex;

/// Skin tone for the bare-hand cuboid.
const SKIN: [f32; 3] = [0.80, 0.60, 0.46];
const HAND_FOV_Y: f32 = 70.0 * std::f32::consts::PI / 180.0;
const HAND_DEPTH: f32 = 1.65;
/// Bare arm sits farther from the view camera than held items so less of it
/// fills the screen.
const BARE_ARM_DEPTH: f32 = 2.10;
const REST_NDC_X: f32 = 0.68;
const REST_NDC_Y: f32 = -0.83;
const VANILLA_ARM_SCALE: f32 = 0.14;
const VANILLA_ARM_ANCHOR_NDC_X: f32 = 0.71;
const VANILLA_ARM_ANCHOR_NDC_Y: f32 = -0.75;

/// Build the first-person hand geometry for `view` into the caller-owned
/// `verts`/`indices` (cleared first, capacity reused — no per-frame allocation),
/// and return its complete clip-space MVP (proj * view * model, with the
/// mining-punch swing / place pop folded in). `aspect` is the framebuffer width /
/// height so the fixed hand perspective matches the screen.
///
/// For a **sprite** held item the model3d geometry is left empty (so the model3d
/// hand pass draws nothing) — the renderer draws the extruded 3D item via the
/// `item3d` pipeline using [`held_sprite`] instead. For `None` (bare arm) and a
/// held block the returned geometry is non-empty; `indices.is_empty()` after the
/// call means there is nothing for the model3d hand pass to draw.
#[cfg(test)]
pub fn build_hand(
    view: &HeldItemView,
    aspect: f32,
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
) -> Mat4 {
    build_hand_lit(view, aspect, DynLight::FULL, 0, verts, indices)
}

pub(super) fn build_hand_lit(
    view: &HeldItemView,
    aspect: f32,
    light: DynLight,
    warm: u8,
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
) -> Mat4 {
    verts.clear();
    indices.clear();

    let base_model = match view.item {
        None => {
            // Unit cube -> vanilla-ish arm dimensions. The first-person pose below
            // applies renderPlayerArm transform sequence.
            push_cube_solid_lit(
                verts,
                indices,
                SKIN,
                Vec3::new(-0.5, -0.5, -0.5),
                1.0,
                light,
            );
            Mat4::from_scale(Vec3::new(4.0, 12.0, 4.0))
        }
        Some(item) => match item.render_kind() {
            ItemRenderKind::BlockCube(block) => {
                // Held block: a corner toward the camera (three-quarter view).
                // Per-face tiles so the furnace shows its front, not four mouths; the
                // chest draws its full inset 3D model instead of a cube.
                if block == Block::Chest {
                    super::chest_model::push_chest_item(
                        verts,
                        indices,
                        Vec3::new(-0.5, -0.5, -0.5),
                        1.0,
                        light,
                    );
                } else {
                    push_block_item_cube_lit_with_state(
                        verts,
                        indices,
                        block,
                        view.block_state,
                        Vec3::new(-0.5, -0.5, -0.5),
                        1.0,
                        light,
                        false,
                    );
                }
                Mat4::from_scale_rotation_translation(
                    Vec3::splat(0.55),
                    Quat::from_rotation_y(0.55) * Quat::from_rotation_x(-0.20),
                    Vec3::ZERO,
                )
            }
            ItemRenderKind::Sprite(_) => {
                // Sprite items are drawn by the renderer via the item3d pipeline
                // (extruded 3D) using `held_sprite`; emit no model3d geometry.
                Mat4::IDENTITY
            }
            ItemRenderKind::Model(_) => {
                // bbmodel items are drawn by the renderer via the item3d pipeline bound
                // to the MODEL atlas (see `held_model`); emit no model3d geometry here.
                Mat4::IDENTITY
            }
        },
    };

    // Warm the whole hand by the block-light it sits in — one post-pass over the
    // freshly built verts — so a held block / bare arm reads warm near a torch or
    // furnace, not just brighter. (A sprite item emits no model3d verts here; it is
    // warmed on the item3d side instead.)
    if warm > 0 {
        let w = warm as f32 / 255.0;
        for v in verts.iter_mut() {
            v.tint = crate::mesh::pack_tint(crate::torch::warm_tint(
                crate::mesh::unpack_tint(v.tint),
                w,
            ));
        }
    }

    let placement = if view.item.is_none() {
        bare_arm_placement(view, aspect)
    } else {
        held_item_placement(view, aspect)
    };
    hand_view_proj(aspect) * placement * base_model
}

/// If `view` holds a sprite-kind item, return its tile + the complete clip-space
/// MVP to draw the EXTRUDED 3D item (built by [`super::item_model`]) in the hand
/// pass at the held three-quarter angle (so the extrusion depth is visible), with
/// the same swing / place-pop animation folded in as the rest of the hand.
/// `None` for bare hand or a held block (those go through [`build_hand`]).
pub fn held_sprite(view: &HeldItemView, aspect: f32) -> Option<(Tile, Mat4)> {
    let item = view.item?;
    let ItemRenderKind::Sprite(tile) = item.render_kind() else {
        return None;
    };
    // First-person hold of a sprite item. The extruded sprite is a unit, origin-
    // centred slab built upright; the item's own [`held_pose`](crate::item::ItemType::held_pose)
    // (item data) tilts it before it's seated in the hand:
    //   * roll (Z), applied FIRST in the sprite's own plane, lays the long axis
    //     diagonally for a swung tool (pickaxes); it's 0 for upright items;
    //   * yaw (Y) then swings the slab past head-on to a steep, near-side-on angle
    //     so the EXTRUDED THICKNESS — not the flat face — reads, for a chunky 3D
    //     look; pitch (X) is a spare tilt, flat for now.
    // `nudge` lifts/shifts it within the shared held anchor so it sits at the
    // screen's lower-right (sprite-only; the anchor is unchanged for held blocks).
    // `s` sizes the slab like a held item.
    let pose = item.held_pose();
    let s = 1.0;
    let nudge = Vec3::new(0.10, 0.10, 0.0);
    let base_model = Mat4::from_scale(Vec3::splat(s))
        * Mat4::from_quat(
            Quat::from_rotation_y(pose.yaw)
                * Quat::from_rotation_x(pose.pitch)
                * Quat::from_rotation_z(pose.roll),
        );
    Some((
        tile,
        hand_view_proj(aspect)
            * held_item_placement(view, aspect)
            * Mat4::from_translation(nudge)
            * base_model,
    ))
}

/// Where the first-person item sits relative to the camera (view units = blocks;
/// camera at origin looking down −Z): the vanilla right-hand anchor, the same point
/// Blockbench's first-person preview seats its `display_area` at (its "monitor"
/// reference: `(9.039, −8.318+24, 20.8)` pixels against a camera at `(0, 24, 32.4)`).
/// Held bbmodel items compose their authored pose about this anchor so the in-game
/// hold matches the Blockbench preview exactly.
const MODEL_HAND_ANCHOR: Vec3 = Vec3::new(9.039 / 16.0, -8.318 / 16.0, -11.6 / 16.0);

/// If `view` holds a bbmodel item, return its kind + the clip-space MVP to draw its
/// actual baked model (centred in a unit cube by the baker) exactly as the authored
/// Blockbench `firstperson_righthand` pose shows it — the model counterpart of
/// [`held_sprite`]. The renderer bakes the geometry (model atlas) and draws it through
/// the item3d pipeline in the hand pass. `None` for a bare hand, a held block, or a
/// sprite.
///
/// The whole pose is DATA: [`ModelInstance::display_from_unit`] rebases the baked
/// unit geometry into the authored display space (blocks about the authored block
/// centre), [`DisplayTransform::base_matrix`] applies the authored
/// translation/rotation/scale/pivots exactly as Blockbench's preview does (raw euler,
/// no mirroring for the right hand), and [`MODEL_HAND_ANCHOR`] seats the result at
/// the vanilla hand point under [`model_hand_view_proj`], the exact camera
/// Blockbench's preview renders with. Editing the pose in Blockbench (then
/// recompiling the `.llblock`) moves the in-game hold, no code.
pub fn held_model(
    view: &HeldItemView,
    aspect: f32,
) -> Option<(crate::block_model::BlockModelKind, Mat4)> {
    let item = view.item?;
    let ItemRenderKind::Model(kind) = item.render_kind() else {
        return None;
    };
    let pose = &crate::block_model::display(kind).firstperson_righthand;
    let model = pose.base_matrix() * crate::block_model::instance(kind).display_from_unit;
    // The swing amplitude was tuned at the legacy HAND_DEPTH; the vanilla anchor is
    // much nearer the camera, so the punch translation scales down proportionally.
    let placement = placement_at(view, MODEL_HAND_ANCHOR, -MODEL_HAND_ANCHOR.z / HAND_DEPTH);
    Some((kind, model_hand_view_proj(aspect) * placement * model))
}

/// Fixed first-person perspective for the hand (independent of the world camera).
fn hand_view_proj(aspect: f32) -> Mat4 {
    let proj = Mat4::perspective_rh(HAND_FOV_Y, aspect.max(0.0001), 0.01, 10.0);
    // Camera at origin looking down -Z; the model sits a short distance ahead.
    let view = Mat4::look_at_rh(Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0), Vec3::Y);
    proj * view
}

/// The camera under which Blockbench's first-person preview is SEEN: its "monitor"
/// reference masks the (wider, `getOptimalFocalLength`) render down to a screen
/// window of black planes — inner edges ±1.65 × ±0.93 at 1.2 units before the camera
/// (display_references `monitor`) — and that window is the vanilla screen. Mapping
/// our framebuffer to that window means a FIXED vertical half-extent slope of
/// `0.93 / 1.2` (≈75.6° vertical), horizontal spanning with aspect, independent of
/// Blockbench's render-canvas fov (window and scene geometry cancel it). Verified
/// against a Blockbench screenshot to <1% (bed features, window-normalized).
fn model_hand_view_proj(aspect: f32) -> Mat4 {
    let vslope: f32 = 0.93 / 1.2;
    let proj = Mat4::perspective_rh(2.0 * vslope.atan(), aspect.max(0.0001), 0.01, 10.0);
    let view = Mat4::look_at_rh(Vec3::ZERO, Vec3::new(0.0, 0.0, -1.0), Vec3::Y);
    proj * view
}

fn view_pos_from_ndc(ndc_x: f32, ndc_y: f32, depth: f32, aspect: f32) -> Vec3 {
    let t = (HAND_FOV_Y * 0.5).tan();
    let aspect = aspect.max(0.0001);
    Vec3::new(ndc_x * aspect * t * depth, ndc_y * t * depth, -depth)
}

fn radians(degrees: f32) -> f32 {
    degrees * std::f32::consts::PI / 180.0
}

/// Static rest orientation of the bare-arm cuboid (no swing): rises from the
/// lower-right toward centre, tilted up, broad back-of-hand face to the camera.
/// This is `renderPlayerArm` transform chain with the swing terms
/// dropped — the punch is layered on separately in [`bare_arm_placement`] so the
/// empty hand jabs forward like a held item instead of wiping sideways.
fn arm_rest_pose() -> Mat4 {
    Mat4::from_translation(Vec3::new(0.64000005, -0.6, -0.71999997))
        * Mat4::from_rotation_y(radians(45.0))
        * Mat4::from_translation(Vec3::new(-1.0, 3.6, 3.5))
        * Mat4::from_rotation_z(radians(120.0))
        * Mat4::from_rotation_x(radians(200.0))
        * Mat4::from_rotation_y(radians(-135.0))
        * Mat4::from_translation(Vec3::new(5.6, 0.0, 0.0))
}

/// Forearm-local shoulder pivot: the bottom (`-Y`) end of the 4x12x4 arm cuboid,
/// which the rest pose sends off-screen toward the lower-right. The punch swings
/// the fist (`+Y` end) about this point so the cuboid hinges like a real arm.
const ARM_SHOULDER_LOCAL: Vec3 = Vec3::new(0.0, -6.0, 0.0);

/// Peak forward-jab angle of the bare-arm punch, in degrees. A rotation about the
/// arm-local `-Z` axis at the shoulder, which (through the rest pose) drives the
/// fist toward screen centre and into the screen — a forward punch, not a sweep.
const ARM_PUNCH_DEG: f32 = 62.0;
/// Secondary roll folded into the punch so the cuboid doesn't hinge flatly; gives
/// the wrist a little twist as the fist comes forward.
const ARM_PUNCH_ROLL_DEG: f32 = 16.0;

/// Bare-arm forward jab about the shoulder, mirroring the held-item punch feel:
/// a fast strike out (peaks early, near `swing` 0.2) easing back to rest at 1.0.
/// `amp` scales the throw (1.0 mining, less for the gentler place jab). Built in
/// the arm-local frame; the caller pivots it at [`ARM_SHOULDER_LOCAL`].
fn arm_punch_rotation(swing: f32, amp: f32) -> Quat {
    let s = swing.clamp(0.0, 1.0);
    // Punchy, asymmetric envelope: 0 at the ends, peaks early at s~=0.2 so the
    // jab snaps out then recovers — same sqrt-eased shape the held item uses.
    let strike = (std::f32::consts::PI * s.sqrt()).sin() * amp;
    Quat::from_rotation_z(radians(-ARM_PUNCH_DEG * strike))
        * Quat::from_rotation_x(radians(-ARM_PUNCH_ROLL_DEG * strike))
}

fn bare_arm_placement(view: &HeldItemView, aspect: f32) -> Mat4 {
    let aspect = aspect.max(0.0001);
    let anchor = view_pos_from_ndc(
        VANILLA_ARM_ANCHOR_NDC_X,
        VANILLA_ARM_ANCHOR_NDC_Y,
        BARE_ARM_DEPTH,
        aspect,
    );
    let rest = Mat4::from_translation(anchor)
        * Mat4::from_scale(Vec3::splat(VANILLA_ARM_SCALE))
        * arm_rest_pose();

    // Hinge the fist about the shoulder so the empty hand punches forward. A
    // placement that just emptied the hand reuses this same jab, softened.
    let punch = Mat4::from_translation(ARM_SHOULDER_LOCAL)
        * Mat4::from_quat(arm_punch_rotation(view.swing, view.swing_scale))
        * Mat4::from_translation(-ARM_SHOULDER_LOCAL);
    rest * punch
}

/// Place held item models in the lower-right and apply the punch animation. The
/// swing serves both mining (full throw) and placing (softer, via `swing_scale`).
fn held_item_placement(view: &HeldItemView, aspect: f32) -> Mat4 {
    let aspect = aspect.max(0.0001);
    let rest = view_pos_from_ndc(REST_NDC_X, REST_NDC_Y, HAND_DEPTH, aspect);
    placement_at(view, rest, 1.0)
}

/// Seat the held item at `rest` (view units) and fold in the mining-punch swing, its
/// translation throw scaled by `throw_scale` so an item seated nearer the camera
/// (the bbmodel anchor) jabs proportionally, not across the whole screen. The EAT
/// pose (mouth carry + nibble) composes here too, so every held render kind
/// (block cube, extruded sprite, bbmodel) eats identically.
fn placement_at(view: &HeldItemView, rest: Vec3, throw_scale: f32) -> Mat4 {
    let mut pos = rest;
    let mut rot = Quat::IDENTITY;

    if view.eat > 0.0 {
        let e = view.eat;
        // Carry the food from its rest anchor up to the MOUTH: toward the
        // screen centre (x, y toward 0) and nearer the camera (z toward 0) —
        // where the first-person face is. Component scaling of the rest anchor
        // keeps the carry aspect- and seat-independent (the bbmodel anchor
        // sits at a different depth than the legacy one). While the food
        // wiggles there, `eat_near` slides the whole mouth point ALONG THE
        // VIEW RAY toward the camera (uniform scale of the view-space point =
        // screen position stays put, the food just looms closer bite by bite).
        let mouth =
            Vec3::new(rest.x * 0.16, rest.y * 0.34, rest.z * 0.74) * (1.0 - 0.28 * view.eat_near);
        let carry = mouth - rest;
        pos += carry * e;
        // Each bite nudges the item a touch further into the mouth (positive
        // half of the oscillator only — bites push, they don't pull).
        let bite = view.eat_bob * e;
        pos += carry.normalize_or_zero() * (0.022 * bite.max(0.0));
        // Tip the item up toward the face and turn it inward, rocking gently
        // with the bite rhythm — the munching read, distinct from any punch.
        let eat_rot = Quat::from_rotation_y(radians(34.0 * e))
            * Quat::from_rotation_x(radians(-56.0 * e + 4.0 * bite))
            * Quat::from_rotation_z(radians(5.0 * bite));
        rot = eat_rot * rot;
    }

    if view.swing > 0.0 {
        let s = view.swing.clamp(0.0, 1.0);
        let amp = view.swing_scale;
        let root_sin = (std::f32::consts::PI * s.sqrt()).sin();
        let swing_sin = (std::f32::consts::PI * s).sin();
        let swing_sq_sin = (std::f32::consts::PI * s * s).sin();

        // Only the translation throw scales with the seat depth; the punch ANGLES are
        // unit-free and keep their full arc.
        pos += amp
            * throw_scale
            * Vec3::new(
                -0.30 * root_sin,
                0.40 * (std::f32::consts::TAU * s.sqrt()).sin(),
                -0.40 * swing_sin,
            );
        let attack = Quat::from_rotation_y(radians(45.0 + amp * swing_sq_sin * -20.0))
            * Quat::from_rotation_z(radians(amp * root_sin * -20.0))
            * Quat::from_rotation_x(radians(amp * root_sin * -80.0))
            * Quat::from_rotation_y(radians(-45.0));
        rot = attack * rot;
    }

    Mat4::from_translation(pos) * Mat4::from_quat(rot)
}

#[cfg(test)]
mod tests;
