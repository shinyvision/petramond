//! First-person held-item / hand geometry.
//!
//! Builds, each frame, the small full-bright model shown in the lower-right of the
//! screen from the flat [`HeldItemView`] prepared by the renderer:
//! - `item == None` -> a skin-colored first-person ARM cuboid
//!   (`block_model::cube_solid`) rising from the lower-right toward centre,
//!   tilted up, broad back-of-hand face to the camera + a darker side visible.
//! - `item` is a block-cube -> the `block_model::cube_textured` block, held with a
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
use super::vanilla_swing::vanilla_swing;
use super::HeldItemView;
use mod_api::animation::PoseSample;
use petramond_mesh::Vertex;
use petramond_world::bbmodel::display_euler_quat;
use petramond_world::block::Block;
use petramond_world::item::ItemRenderKind;
use petramond_world::tile::Tile;

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
    build_hand_lit(view, aspect, DynLight::FULL, verts, indices)
}

pub(super) fn build_hand_lit(
    view: &HeldItemView,
    aspect: f32,
    light: DynLight,
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

    // Instance-data tint (`petramond:tint`): multiply the held ITEM's verts.
    // Never the bare arm — its solid path uses tint as the final skin color.
    if view.item.is_some() {
        super::item_model::dye_block_verts(verts, view.variant);
    }

    let placement = if view.item.is_none() {
        bare_arm_placement(view, aspect)
    } else {
        held_item_placement(view, aspect)
    };
    hand_view_proj(aspect) * placement * base_model
}

/// Mirror a rigid placement across the view-space YZ plane by CONJUGATION:
/// `S · M · S` with `S = diag(-1, 1, 1)`. Preserves the determinant, so
/// geometry keeps its winding and its texturing; only the POSE mirrors to the
/// screen's left. Used ONLY for the held block cube — a cube's three-quarter
/// view survives it because the geometry is symmetric. Anything with an
/// asymmetric pose or silhouette (tool sprites, bbmodels) must use
/// [`reflect_x`] instead: conjugation shows a DIFFERENT orientation (negated
/// rotations of unmirrored geometry — the 2026-08-21 hoe-facing-the-player /
/// invisible-pottery-table bug), not the mirror image.
fn mirror_x(m: Mat4) -> Mat4 {
    let s = Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0));
    s * m * s
}

/// TRUE reflection across the view-space YZ plane: `S · M`. The left-hand
/// image is EXACTLY the right-hand image flipped horizontally — which is what
/// Blockbench's `firstperson_lefthand` preview shows (the authored left pose
/// rendered in a mirrored view space), and therefore the target for every
/// authored pose. Flips triangle winding, so only double-sided (cull `None`)
/// paths may draw it — the item3d held pipeline is.
fn reflect_x(m: Mat4) -> Mat4 {
    Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)) * m
}

/// The OFF-hand twin of [`build_hand_lit`]: the same geometry, seated at the
/// screen's lower-LEFT by mirroring the placement (see [`mirror_x`]). An empty
/// off-hand builds NOTHING — there is no bare left arm; the left hand appears
/// exactly while the off-hand slot holds an item.
pub(super) fn build_off_hand_lit(
    view: &HeldItemView,
    aspect: f32,
    light: DynLight,
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
) -> Mat4 {
    verts.clear();
    indices.clear();
    let Some(item) = view.item else {
        return Mat4::IDENTITY;
    };
    let base_model = match item.render_kind() {
        ItemRenderKind::BlockCube(block) => {
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
        // Sprites and bbmodels ride the item3d pipeline (`held_sprite_off` /
        // `held_model_off`); no model3d geometry here.
        ItemRenderKind::Sprite(_) | ItemRenderKind::Model(_) => Mat4::IDENTITY,
    };
    super::item_model::dye_block_verts(verts, view.variant);
    hand_view_proj(aspect) * mirror_x(held_item_placement(view, aspect) * base_model)
}

/// If `view` holds a sprite-kind item, return its tile + the complete clip-space
/// MVP to draw the EXTRUDED 3D item (built by [`super::item_model`]) in the hand
/// pass at the held three-quarter angle (so the extrusion depth is visible), with
/// the same swing / place-pop animation folded in as the rest of the hand.
/// `None` for bare hand or a held block (those go through `build_hand`).
pub fn held_sprite(view: &HeldItemView, aspect: f32) -> Option<(Tile, Mat4)> {
    let item = view.item?;
    let ItemRenderKind::Sprite(tile) = item.render_kind() else {
        return None;
    };
    // First-person hold of a sprite item. The extruded sprite is a unit, origin-
    // centred slab built upright; the held STACK's own authored hold
    // (`view.hold`, item data — never a display stand-in's) tilts it before
    // it's seated in the hand:
    // * roll (Z), applied FIRST in the sprite's own plane, lays the long axis
    // diagonally for a swung tool (pickaxes); it's 0 for upright items;
    // * yaw (Y) then swings the slab past head-on to a steep, near-side-on angle
    // so the EXTRUDED THICKNESS — not the flat face — reads, for a chunky 3D
    // look; pitch (X) is a spare tilt, flat for now.
    // `nudge` lifts/shifts it within the shared held anchor so it sits at the
    // screen's lower-right (sprite-only; the anchor is unchanged for held blocks).
    // `s` sizes the slab like a held item.
    let pose = view.hold;
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

/// The OFF-hand twin of [`held_sprite`]: the right-hand composition, truly
/// reflected ([`reflect_x`] — a symmetric perspective commutes with the flip,
/// so this is exactly the right-hand MVP with clip-space x negated). The left
/// hand shows the MIRROR IMAGE, so a tool's rolled art still points its head
/// forward (conjugation pointed it at the player, the 2026-08-21 hoe bug).
/// Near-symmetric art (flowers, torches) reads identically either way.
pub fn held_sprite_off(view: &HeldItemView, aspect: f32) -> Option<(Tile, Mat4)> {
    let (tile, mvp) = held_sprite(view, aspect)?;
    Some((tile, reflect_x(mvp)))
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
/// The whole pose is DATA: `ModelInstance::display_from_unit` rebases the baked
/// unit geometry into the authored display space (blocks about the authored block
/// centre), `DisplayTransform::base_matrix` applies the authored
/// translation/rotation/scale/pivots exactly as Blockbench's preview does (raw euler,
/// no mirroring for the right hand), and `MODEL_HAND_ANCHOR` seats the result at
/// the vanilla hand point under `model_hand_view_proj`, the exact camera
/// Blockbench's preview renders with. Editing the pose in Blockbench (then
/// recompiling the `.llblock`) moves the in-game hold, no code.
pub fn held_model(
    view: &HeldItemView,
    aspect: f32,
) -> Option<(petramond_world::block_model::BlockModelKind, Mat4)> {
    let item = view.item?;
    let ItemRenderKind::Model(kind) = item.render_kind() else {
        return None;
    };
    let pose = &petramond_world::block_model::display(kind).firstperson_righthand;
    let model = pose.base_matrix() * petramond_world::block_model::instance(kind).display_from_unit;
    // The swing amplitude was tuned at the legacy HAND_DEPTH; the vanilla anchor is
    // much nearer the camera, so the punch translation scales down proportionally.
    let placement = placement_at(view, MODEL_HAND_ANCHOR, -MODEL_HAND_ANCHOR.z / HAND_DEPTH);
    Some((kind, model_hand_view_proj(aspect) * placement * model))
}

/// The OFF-hand twin of [`held_model`] — EXACTLY Blockbench's lefthand
/// preview composition (display_mode.js): the display area seats at the
/// MIRRORED hand anchor (`mirror_x(placement)` — a conjugation, so the jab
/// dynamics mirror too), the pose is the slot's values with `translation.x`
/// / `rotation.y` / `rotation.z` NEGATED ([`DisplayTransform::left_hand`] —
/// applied to an authored `firstperson_lefthand` too, exactly as Blockbench
/// previews it), and the GEOMETRY (with its `display_from_unit` rebase) is
/// untouched — no reflection. A whole-chain reflection instead mirrors each
/// model's own authored x-offset, which pushed the pottery table off the
/// left edge and the forging furnace to the right (2026-08-21 round 3).
pub fn held_model_off(
    view: &HeldItemView,
    aspect: f32,
) -> Option<(petramond_world::block_model::BlockModelKind, Mat4)> {
    let item = view.item?;
    let ItemRenderKind::Model(kind) = item.render_kind() else {
        return None;
    };
    let display = petramond_world::block_model::display(kind);
    let pose = display
        .firstperson_lefthand
        .as_ref()
        .unwrap_or(&display.firstperson_righthand)
        .left_hand();
    let model = pose.base_matrix() * petramond_world::block_model::instance(kind).display_from_unit;
    let placement = placement_at(view, MODEL_HAND_ANCHOR, -MODEL_HAND_ANCHOR.z / HAND_DEPTH);
    Some((
        kind,
        model_hand_view_proj(aspect) * mirror_x(placement) * model,
    ))
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

/// One sampled instant of a vanilla swing channel, amplitude-scaled: the
/// authored degrees and px scale LINEARLY with `swing_scale`, so a softer
/// place jab is the same arc at a smaller angle, not a different shape.
fn scaled_sample(sample: PoseSample, amp: f32) -> PoseSample {
    let s = |v: [f32; 3]| [v[0] * amp, v[1] * amp, v[2] * amp];
    PoseSample {
        rotation: s(sample.rotation),
        translation: s(sample.translation),
        origin: sample.origin,
    }
}

/// One sampled pose instant as a transform: the rotation applied about the
/// sample's own pivot (`origin`), then the sample's translation. A curve
/// that keys an origin hinges there; one that keys none rotates in place
/// and carries its motion in translation — both are the author's choice,
/// and this sandwich is what makes either read back exactly as keyed.
fn pose_matrix(s: PoseSample) -> Mat4 {
    let origin = Vec3::from(s.origin);
    Mat4::from_translation(origin + Vec3::from(s.translation))
        * Mat4::from_quat(display_euler_quat(Vec3::from(s.rotation)))
        * Mat4::from_translation(-origin)
}

/// Bare-arm jab: the authored `hand_arm.json` channel played through
/// [`pose_matrix`] in the arm's local frame. `amp` scales the throw (1.0
/// mining, less for the gentler place jab). A missing/refused file jabs
/// nothing — rest.
fn arm_punch(swing: f32, amp: f32) -> Mat4 {
    let Some(curve) = &vanilla_swing().arm else {
        return Mat4::IDENTITY;
    };
    pose_matrix(scaled_sample(curve.sample(swing.clamp(0.0, 1.0)), amp))
}

/// The hand's walking sway as a view-space translation. Already lagged and
/// scaled by the animator; the `-y` is because view space grows upward while
/// the bob's `up` channel is a rise.
fn bob_offset(view: &HeldItemView) -> Vec3 {
    Vec3::new(view.bob[0], view.bob[1], 0.0)
}

fn bare_arm_placement(view: &HeldItemView, aspect: f32) -> Mat4 {
    let aspect = aspect.max(0.0001);
    let anchor = view_pos_from_ndc(
        VANILLA_ARM_ANCHOR_NDC_X,
        VANILLA_ARM_ANCHOR_NDC_Y,
        BARE_ARM_DEPTH,
        aspect,
    );
    let rest = Mat4::from_translation(anchor + bob_offset(view))
        * Mat4::from_scale(Vec3::splat(VANILLA_ARM_SCALE))
        * arm_rest_pose();

    // The authored jab plays in the arm's local frame. A placement that
    // just emptied the hand reuses it, softened.
    rest * arm_punch(view.swing, view.swing_scale)
}

/// Place held item models in the lower-right and apply the punch animation. The
/// swing serves both mining (full throw) and placing (softer, via `swing_scale`).
fn held_item_placement(view: &HeldItemView, aspect: f32) -> Mat4 {
    let aspect = aspect.max(0.0001);
    let rest = view_pos_from_ndc(REST_NDC_X, REST_NDC_Y, HAND_DEPTH, aspect);
    placement_at(view, rest, 1.0)
}

/// Seat the held item at `rest` (view units) and fold in the mining-punch
/// swing, its translation throw scaled by `throw_scale` so an item seated
/// nearer the camera (the bbmodel anchor) jabs proportionally, not across
/// the whole screen. The claimed held-pose offset (base stance) and the EAT
/// pose (mouth carry + nibble) compose here too, so every held render kind
/// (block cube, extruded sprite, bbmodel) poses identically.
fn placement_at(view: &HeldItemView, rest: Vec3, throw_scale: f32) -> Mat4 {
    let mut pos = rest + bob_offset(view);
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
        if let Some(curve) = &vanilla_swing().held {
            // The authored held-item channel (hand_swing.json), amplitude-
            // scaled. Only the translation throw scales with the seat depth;
            // the punch ANGLES are unit-free and keep their full arc. The
            // file's px are 1/16-block, and view space here is blocks.
            let s = scaled_sample(curve.sample(view.swing.clamp(0.0, 1.0)), view.swing_scale);
            pos += Vec3::from(s.translation) / 16.0 * throw_scale;
            rot = display_euler_quat(Vec3::from(s.rotation)) * rot;
        }
    }

    // The claimed pose composes INSIDE the seat, so the punch and the eat
    // carry a posed item instead of fighting it, and its 1/16-block
    // translation lands in view space (which is blocks — the hand's own
    // camera puts the anchor at `MODEL_HAND_ANCHOR` in block units).
    //
    // Every off-hand path mirrors a chain that contains this one — the block
    // cube and the bbmodel by CONJUGATION, the sprite by true reflection —
    // and conjugating a display transform by the x-flip is exactly
    // `DisplayTransform::left_hand`, so one authored pose reads correctly
    // from either fist with no per-hand rule here.
    Mat4::from_translation(pos) * Mat4::from_quat(rot) * view.pose.first_person.base_matrix()
}

#[cfg(test)]
mod tests;
