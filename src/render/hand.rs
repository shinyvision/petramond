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
            // applies Minecraft's renderPlayerArm transform sequence.
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
            v.tint = crate::torch::warm_tint(v.tint, w);
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
/// This is Minecraft's `renderPlayerArm` transform chain with the swing terms
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
        let mouth = Vec3::new(rest.x * 0.16, rest.y * 0.34, rest.z * 0.74)
            * (1.0 - 0.28 * view.eat_near);
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
mod tests {
    use super::*;
    use crate::item::ItemType;

    #[test]
    fn bare_hand_builds_solid_cuboid() {
        let view = HeldItemView {
            item: None,
            block_state: Default::default(),
            swing: 0.0,
            swing_scale: 1.0,
            eat: 0.0,
            eat_bob: 0.0,
            eat_near: 0.0,
        };
        let (mut v, mut i) = (Vec::new(), Vec::new());
        build_hand(&view, 16.0 / 9.0, &mut v, &mut i);
        assert!(!i.is_empty());
        // Solid cuboid = one cube (24 verts / 36 indices).
        assert_eq!(v.len(), 24);
        assert_eq!(i.len(), 36);
        // Every vertex carries the solid-color flag and the skin tint.
        for vert in &v {
            assert_eq!(
                vert.packed & super::super::SOLID_COLOR_FLAG,
                super::super::SOLID_COLOR_FLAG
            );
            assert_eq!(vert.tint, SKIN);
        }
    }

    #[test]
    fn held_block_builds_textured_cube() {
        let view = HeldItemView {
            item: Some(ItemType::OakLog),
            block_state: Default::default(),
            swing: 0.0,
            swing_scale: 1.0,
            eat: 0.0,
            eat_bob: 0.0,
            eat_near: 0.0,
        };
        let (mut v, mut i) = (Vec::new(), Vec::new());
        build_hand(&view, 16.0 / 9.0, &mut v, &mut i);
        assert_eq!(v.len(), 24);
        assert_eq!(i.len(), 36);
        // Textured path never sets the solid flag.
        for vert in &v {
            assert_eq!(vert.packed & super::super::SOLID_COLOR_FLAG, 0);
        }
    }

    #[test]
    fn lit_hand_packs_sampled_skylight() {
        let view = HeldItemView {
            item: Some(ItemType::Stone),
            block_state: Default::default(),
            swing: 0.0,
            swing_scale: 1.0,
            eat: 0.0,
            eat_bob: 0.0,
            eat_near: 0.0,
        };
        let (mut v, mut i) = (Vec::new(), Vec::new());

        build_hand_lit(
            &view,
            16.0 / 9.0,
            DynLight { sky: 9, block: 5 },
            0,
            &mut v,
            &mut i,
        );

        assert!(!v.is_empty());
        for vert in &v {
            assert_eq!((vert.packed >> 23) & 0x3F, 9, "sky channel in word 1");
            assert_eq!(vert.packed2 & 0x3F, 5, "block channel in word 2");
        }
    }

    #[test]
    fn held_sprite_emits_no_model3d_geometry() {
        // Sprite items are drawn by the renderer via the item3d (extruded)
        // pipeline, NOT the model3d hand pass, so build_hand emits nothing.
        let view = HeldItemView {
            item: Some(ItemType::Poppy),
            block_state: Default::default(),
            swing: 0.0,
            swing_scale: 1.0,
            eat: 0.0,
            eat_bob: 0.0,
            eat_near: 0.0,
        };
        let (mut v, mut i) = (Vec::new(), Vec::new());
        build_hand(&view, 16.0 / 9.0, &mut v, &mut i);
        assert!(v.is_empty(), "sprite hand emits no model3d verts");
        assert!(i.is_empty(), "sprite hand emits no model3d indices");
    }

    /// CPU-rasterize one held-model view (perspective divide, z-buffer, model-atlas
    /// sampling — exactly what the item3d hand pass draws) into row `row` of a stacked
    /// `w`×`h`-per-row RGB canvas. Shared by the preview harnesses below.
    fn raster_held_cell(
        kind: crate::block_model::BlockModelKind,
        mvp: Mat4,
        (w, h): (usize, usize),
        row: usize,
        color: &mut [u8],
    ) {
        use crate::render::lighting::{DynLight, LightEnv};
        let (atlas_rgba, aw, ah) = crate::block_model::atlas().texture();
        let (mut verts, mut indices) = (Vec::new(), Vec::new());
        crate::render::item_model::build_block_model_item(
            kind,
            Mat4::IDENTITY,
            DynLight::FULL,
            LightEnv::IDENTITY,
            0,
            None,
            &mut verts,
            &mut indices,
        );
        let mut zbuf = vec![f32::INFINITY; w * h];
        let project = |p: [f32; 3]| -> Option<[f32; 3]> {
            let c = mvp * glam::Vec4::new(p[0], p[1], p[2], 1.0);
            if c.w <= 1e-6 {
                return None;
            }
            let n = c / c.w;
            Some([
                (n.x * 0.5 + 0.5) * w as f32,
                (1.0 - (n.y * 0.5 + 0.5)) * h as f32,
                n.z,
            ])
        };
        for tri in indices.chunks_exact(3) {
            let vtx = [
                verts[tri[0] as usize],
                verts[tri[1] as usize],
                verts[tri[2] as usize],
            ];
            let (Some(s0), Some(s1), Some(s2)) = (
                project(vtx[0].pos),
                project(vtx[1].pos),
                project(vtx[2].pos),
            ) else {
                continue;
            };
            let s = [s0, s1, s2];
            let (x0, y0, x1, y1, x2, y2) = (s[0][0], s[0][1], s[1][0], s[1][1], s[2][0], s[2][1]);
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
                    let u = w0 * vtx[0].uv[0] + w1 * vtx[1].uv[0] + w2 * vtx[2].uv[0];
                    let v = w0 * vtx[0].uv[1] + w1 * vtx[1].uv[1] + w2 * vtx[2].uv[1];
                    let tx = (u * aw as f32).clamp(0.0, aw as f32 - 1.0) as u32;
                    let ty = (v * ah as f32).clamp(0.0, ah as f32 - 1.0) as u32;
                    let ti = ((ty * aw + tx) * 4) as usize;
                    if atlas_rgba[ti + 3] < 128 {
                        continue;
                    }
                    let shade = w0 * vtx[0].shade + w1 * vtx[1].shade + w2 * vtx[2].shade;
                    zbuf[li] = z;
                    let o = ((row * h + y) * w + x) * 3;
                    color[o] = (atlas_rgba[ti] as f32 * shade).min(255.0) as u8;
                    color[o + 1] = (atlas_rgba[ti + 1] as f32 * shade).min(255.0) as u8;
                    color[o + 2] = (atlas_rgba[ti + 2] as f32 * shade).min(255.0) as u8;
                }
            }
        }
    }

    /// Visual preview harness (NOT an assertion): rasterizes each held bbmodel item via
    /// the REAL `held_model` MVP into a stacked PNG, so the in-hand pose can be checked
    /// against Blockbench's first-person preview without launching the game.
    /// Run: `cargo test --lib -- --ignored --nocapture render_held_model_preview`.
    /// Writes /tmp/held_model.png.
    #[test]
    #[ignore = "visual preview harness; run explicitly to regenerate /tmp/held_model.png"]
    fn render_held_model_preview() {
        let items = [
            ("WoodenBucket", ItemType::WoodenBucket),
            ("WaterBucket", ItemType::WaterBucket),
            ("FurnitureWorkbench", ItemType::FurnitureWorkbench),
            ("Bed", ItemType::Bed),
        ];
        let (w, h) = (940usize, 530usize);
        let aspect = w as f32 / h as f32;
        let bg = [30u8, 32, 38];
        let gh = h * items.len();
        let mut color = vec![0u8; w * gh * 3];
        for px in color.chunks_mut(3) {
            px.copy_from_slice(&bg);
        }
        for (row, (label, item)) in items.iter().enumerate() {
            let view = HeldItemView {
                item: Some(*item),
                block_state: Default::default(),
                swing: 0.0,
                swing_scale: 1.0,
                eat: 0.0,
                eat_bob: 0.0,
                eat_near: 0.0,
            };
            let (kind, mvp) = held_model(&view, aspect).expect("model item");
            raster_held_cell(kind, mvp, (w, h), row, &mut color);
            println!("row {row}: {label}");
        }
        image::save_buffer(
            "/tmp/held_model.png",
            &color,
            w as u32,
            gh as u32,
            image::ColorType::Rgb8,
        )
        .expect("save png");
        println!("wrote /tmp/held_model.png ({w}x{gh}, one row per item)");
    }

    #[test]
    fn held_sprite_reports_tile_and_mvp() {
        // held_sprite drives the extruded item3d draw; it must report the sprite
        // tile (and a finite MVP) for a sprite item and None otherwise.
        let poppy = HeldItemView {
            item: Some(ItemType::Poppy),
            block_state: Default::default(),
            swing: 0.0,
            swing_scale: 1.0,
            eat: 0.0,
            eat_bob: 0.0,
            eat_near: 0.0,
        };
        let (tile, mvp) = held_sprite(&poppy, 16.0 / 9.0).expect("sprite reports a tile");
        assert_eq!(tile, crate::atlas::Tile::named("poppy"));
        assert!(mvp.to_cols_array().iter().all(|f| f.is_finite()));
        // Bare hand + held block return None (they go through build_hand).
        let bare = HeldItemView {
            item: None,
            block_state: Default::default(),
            ..poppy
        };
        let block = HeldItemView {
            item: Some(ItemType::Stone),
            block_state: Default::default(),
            ..poppy
        };
        assert!(held_sprite(&bare, 1.5).is_none());
        assert!(held_sprite(&block, 1.5).is_none());
    }

    #[test]
    fn build_hand_reuses_buffers_without_growth() {
        // The hand buffers are cleared + refilled each call, never reallocated.
        let block = HeldItemView {
            item: Some(ItemType::Stone),
            block_state: Default::default(),
            swing: 0.0,
            swing_scale: 1.0,
            eat: 0.0,
            eat_bob: 0.0,
            eat_near: 0.0,
        };
        let bare = HeldItemView {
            item: None,
            block_state: Default::default(),
            swing: 0.0,
            swing_scale: 1.0,
            eat: 0.0,
            eat_bob: 0.0,
            eat_near: 0.0,
        };
        let (mut v, mut i) = (Vec::new(), Vec::new());
        build_hand(&block, 1.5, &mut v, &mut i);
        let (vcap, icap) = (v.capacity(), i.capacity());
        // Same vert/index count for the bare hand, so capacity is unchanged.
        build_hand(&bare, 1.5, &mut v, &mut i);
        assert_eq!(v.capacity(), vcap, "hand vert buffer reused");
        assert_eq!(i.capacity(), icap, "hand index buffer reused");
    }

    #[derive(Copy, Clone)]
    struct Bounds {
        min_x: f32,
        max_x: f32,
        min_y: f32,
        max_y: f32,
    }

    fn ndc_bounds(mvp: Mat4) -> Bounds {
        use glam::Vec4;

        let mut bounds = Bounds {
            min_x: f32::INFINITY,
            max_x: f32::NEG_INFINITY,
            min_y: f32::INFINITY,
            max_y: f32::NEG_INFINITY,
        };
        for &x in &[-0.5f32, 0.5] {
            for &y in &[-0.5f32, 0.5] {
                for &z in &[-0.5f32, 0.5] {
                    let c = mvp * Vec4::new(x, y, z, 1.0);
                    let ndc = c / c.w;
                    bounds.min_x = bounds.min_x.min(ndc.x);
                    bounds.max_x = bounds.max_x.max(ndc.x);
                    bounds.min_y = bounds.min_y.min(ndc.y);
                    bounds.max_y = bounds.max_y.max(ndc.y);
                }
            }
        }
        bounds
    }

    fn projected_face_area(mvp: Mat4, face: [Vec3; 4]) -> f32 {
        let mut p = [[0.0f32; 2]; 4];
        for (dst, src) in p.iter_mut().zip(face) {
            let c = mvp * src.extend(1.0);
            let ndc = c / c.w;
            *dst = [ndc.x, ndc.y];
        }
        let mut area = 0.0;
        for i in 0..4 {
            let a = p[i];
            let b = p[(i + 1) & 3];
            area += a[0] * b[1] - b[0] * a[1];
        }
        area.abs() * 0.5
    }

    #[test]
    fn bare_hand_rest_is_anchored_lower_right() {
        let screens: [(u32, u32); 4] = [(1280, 720), (1920, 1080), (2560, 1440), (3840, 2160)];
        let view = HeldItemView {
            item: None,
            block_state: Default::default(),
            swing: 0.0,
            swing_scale: 1.0,
            eat: 0.0,
            eat_bob: 0.0,
            eat_near: 0.0,
        };
        let (mut v, mut i) = (Vec::new(), Vec::new());
        for screen in screens {
            let aspect = screen.0 as f32 / screen.1 as f32;
            let bounds = ndc_bounds(build_hand(&view, aspect, &mut v, &mut i));
            assert!(
                bounds.min_x > 0.42,
                "hand starts too far left on {screen:?}: {}",
                bounds.min_x
            );
            assert!(
                bounds.max_x > 0.86,
                "hand is almost hidden off the right side on {screen:?}: {}",
                bounds.max_x
            );
            assert!(
                bounds.min_y < -0.95,
                "hand bottom should sit offscreen on {screen:?}: {}",
                bounds.min_y
            );
            assert!(
                bounds.max_y < -0.20,
                "hand is too high on {screen:?}: {}",
                bounds.max_y
            );
            assert!(
                bounds.max_y > -0.70,
                "hand is almost hidden below the screen on {screen:?}: {}",
                bounds.max_y
            );
        }
    }

    #[test]
    fn bare_hand_rest_does_not_show_large_fist_cap() {
        let view = HeldItemView {
            item: None,
            block_state: Default::default(),
            swing: 0.0,
            swing_scale: 1.0,
            eat: 0.0,
            eat_bob: 0.0,
            eat_near: 0.0,
        };
        let (mut v, mut i) = (Vec::new(), Vec::new());
        let mvp = build_hand(&view, 16.0 / 9.0, &mut v, &mut i);

        let pos_x = [
            Vec3::new(0.5, -0.5, 0.5),
            Vec3::new(0.5, -0.5, -0.5),
            Vec3::new(0.5, 0.5, -0.5),
            Vec3::new(0.5, 0.5, 0.5),
        ];
        let neg_x = [
            Vec3::new(-0.5, -0.5, -0.5),
            Vec3::new(-0.5, -0.5, 0.5),
            Vec3::new(-0.5, 0.5, 0.5),
            Vec3::new(-0.5, 0.5, -0.5),
        ];
        let pos_y = [
            Vec3::new(-0.5, 0.5, 0.5),
            Vec3::new(0.5, 0.5, 0.5),
            Vec3::new(0.5, 0.5, -0.5),
            Vec3::new(-0.5, 0.5, -0.5),
        ];
        let neg_y = [
            Vec3::new(-0.5, -0.5, -0.5),
            Vec3::new(0.5, -0.5, -0.5),
            Vec3::new(0.5, -0.5, 0.5),
            Vec3::new(-0.5, -0.5, 0.5),
        ];
        let pos_z = [
            Vec3::new(-0.5, -0.5, 0.5),
            Vec3::new(0.5, -0.5, 0.5),
            Vec3::new(0.5, 0.5, 0.5),
            Vec3::new(-0.5, 0.5, 0.5),
        ];
        let neg_z = [
            Vec3::new(0.5, -0.5, -0.5),
            Vec3::new(-0.5, -0.5, -0.5),
            Vec3::new(-0.5, 0.5, -0.5),
            Vec3::new(0.5, 0.5, -0.5),
        ];

        let top = projected_face_area(mvp, pos_z).max(projected_face_area(mvp, neg_z));
        let side = projected_face_area(mvp, pos_x).max(projected_face_area(mvp, neg_x));
        let end_cap = projected_face_area(mvp, pos_y).max(projected_face_area(mvp, neg_y));
        assert!(
            side > end_cap * 1.5,
            "vanilla arm should not expose a dominant fist/end cap: side={side}, cap={end_cap}"
        );
        assert!(
            top > end_cap * 1.8,
            "vanilla arm top/back face should dominate fist/end cap: top={top}, cap={end_cap}"
        );
    }

    #[test]
    fn swing_punches_forward_instead_of_right_hooking() {
        let aspect = 16.0 / 9.0;
        let rest_view = HeldItemView {
            item: None,
            block_state: Default::default(),
            swing: 0.0,
            swing_scale: 1.0,
            eat: 0.0,
            eat_bob: 0.0,
            eat_near: 0.0,
        };
        let early_view = HeldItemView {
            swing: 0.25,
            ..rest_view
        };
        let mid_view = HeldItemView {
            swing: 0.5,
            ..rest_view
        };
        let late_view = HeldItemView {
            swing: 0.75,
            ..rest_view
        };
        let done_view = HeldItemView {
            swing: 1.0,
            ..rest_view
        };

        let rest = bare_arm_placement(&rest_view, aspect).transform_point3(Vec3::ZERO);
        let early = bare_arm_placement(&early_view, aspect).transform_point3(Vec3::ZERO);
        let mid = bare_arm_placement(&mid_view, aspect).transform_point3(Vec3::ZERO);
        let late = bare_arm_placement(&late_view, aspect).transform_point3(Vec3::ZERO);
        let done = bare_arm_placement(&done_view, aspect).transform_point3(Vec3::ZERO);

        assert!(
            early.x < rest.x,
            "vanilla swing should move toward center, not right: {early:?} vs {rest:?}"
        );
        assert!(
            mid.x < rest.x,
            "mid-swing should not hook right: {mid:?} vs {rest:?}"
        );
        assert!(
            mid.z < rest.z,
            "mid-swing should punch forward into the target block: {mid:?} vs {rest:?}"
        );
        assert!(
            late.x < rest.x,
            "late swing should still be returning from a forward/center punch, not finishing right"
        );
        assert!(
            (done - rest).length() < 0.001,
            "swing phase 1.0 should return to rest"
        );

        let rest_up = bare_arm_placement(&rest_view, aspect)
            .transform_vector3(Vec3::Y)
            .normalize();
        let mid_up = bare_arm_placement(&mid_view, aspect)
            .transform_vector3(Vec3::Y)
            .normalize();
        assert!(
            rest_up.dot(mid_up) < 0.86,
            "swing should rotate around the pivot, not just translate"
        );
    }

    #[test]
    fn arm_punch_hinges_the_fist_forward_from_the_shoulder() {
        let aspect = 16.0 / 9.0;
        let fist_local = Vec3::new(0.0, 6.0, 0.0); // +Y end of the arm cuboid
        let view = |swing| HeldItemView {
            item: None,
            block_state: Default::default(),
            swing,
            swing_scale: 1.0,
            eat: 0.0,
            eat_bob: 0.0,
            eat_near: 0.0,
        };
        let fist = |swing| bare_arm_placement(&view(swing), aspect).transform_point3(fist_local);
        let shoulder =
            |swing| bare_arm_placement(&view(swing), aspect).transform_point3(ARM_SHOULDER_LOCAL);

        let rest = fist(0.0);
        let mid = fist(0.5);
        // The fist drives toward screen centre (smaller x) and into the screen
        // (more negative z): a forward punch, not the old sideways wipe.
        assert!(
            mid.x < rest.x,
            "fist should swing toward center: {mid:?} vs {rest:?}"
        );
        assert!(
            mid.z < rest.z,
            "fist should punch into the screen: {mid:?} vs {rest:?}"
        );

        // The shoulder pivot barely moves — the arm hinges, it doesn't slide.
        assert!(
            (shoulder(0.5) - shoulder(0.0)).length() < 1e-4,
            "shoulder is the fixed pivot of the punch"
        );

        // The strike returns home: phase 1.0 matches the rest pose.
        assert!(
            (fist(1.0) - rest).length() < 1e-3,
            "punch should ease back to rest at phase 1.0"
        );
    }

    #[test]
    fn swing_and_place_change_the_mvp() {
        let rest = HeldItemView {
            item: Some(ItemType::Stone),
            block_state: Default::default(),
            swing: 0.0,
            swing_scale: 1.0,
            eat: 0.0,
            eat_bob: 0.0,
            eat_near: 0.0,
        };
        let mid_punch = HeldItemView { swing: 0.5, ..rest };
        // A reduced amplitude (< 1.0) stands in for the softer place jab so the
        // resulting MVP differs from the full mining punch.
        let mid_place = HeldItemView {
            swing: 0.5,
            swing_scale: 0.62,
            ..rest
        };
        let (mut v, mut i) = (Vec::new(), Vec::new());
        let a = build_hand(&rest, 1.5, &mut v, &mut i);
        let b = build_hand(&mid_punch, 1.5, &mut v, &mut i);
        let c = build_hand(&mid_place, 1.5, &mut v, &mut i);
        assert_ne!(a, b, "mid-swing must move the hand");
        // The softer place jab also moves the hand, but less than a full punch.
        assert_ne!(a, c, "place swing must move the hand");
        assert_ne!(b, c, "the place jab is softer than the mining punch");
    }

    /// Visual preview harness (NOT an assertion): rasterizes held sprite items via
    /// the REAL `held_sprite` MVP (so it reflects each item's per-item `held_pose`)
    /// to PNGs — pose looks right in source but wrong on screen, so render to
    /// verify. Run: `cargo test --lib -- --ignored --nocapture render_held_item_preview`.
    /// Writes /tmp/held_<item>.png (full 16:9) + _zoom.png (auto-framed 2x).
    #[test]
    #[ignore = "visual preview harness; run explicitly to regenerate /tmp PNGs"]
    fn render_held_item_preview() {
        use crate::atlas::tile_uv;
        use crate::item::ItemType;
        use glam::Vec4;

        // (item, texture, eat blend, bite phase, approach) — the eat rows
        // preview the mouth-carry pose (mid-carry, full, and full at the end
        // of the toward-the-camera approach).
        let targets = [
            (ItemType::StonePickaxe, "stone_pickaxe.png", 0.0, 0.0, 0.0),
            (ItemType::Poppy, "poppy.png", 0.0, 0.0, 0.0),
            (ItemType::Poppy, "poppy.png", 0.5, 0.6, 0.0),
            (ItemType::Poppy, "poppy.png", 1.0, 0.9, 0.0),
            (ItemType::Poppy, "poppy.png", 1.0, -0.9, 1.0),
        ];
        const W: usize = 1280;
        const H: usize = 720;
        let aspect = W as f32 / H as f32;
        let bg = [74u8, 100, 64];

        for (item, file, eat, eat_bob, eat_near) in targets {
            let view = HeldItemView {
                item: Some(item),
                block_state: Default::default(),
                swing: 0.0,
                swing_scale: 1.0,
                eat,
                eat_bob,
                eat_near,
            };
            let (tile, mvp) = held_sprite(&view, aspect).expect("sprite item");
            let mut verts = Vec::new();
            crate::render::item_model::build_extruded_item_lit(
                tile,
                DynLight::FULL,
                crate::render::lighting::LightEnv::IDENTITY,
                &mut verts,
            );
            let src = format!("{}/assets/textures/{}", env!("CARGO_MANIFEST_DIR"), file);
            let img = image::open(&src).expect("texture").to_rgba8();
            let (tw, th) = img.dimensions();
            let [au0, av0, au1, av1] = tile_uv(tile);

            let mut color = vec![0u8; W * H * 3];
            for px in color.chunks_mut(3) {
                px.copy_from_slice(&bg);
            }
            let mut zbuf = vec![f32::INFINITY; W * H];
            let (mut bx0, mut by0, mut bx1, mut by1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            let project = |p: [f32; 3]| -> [f32; 4] {
                let clip = mvp * Vec4::new(p[0], p[1], p[2], 1.0);
                let invw = 1.0 / clip.w;
                [
                    (clip.x * invw * 0.5 + 0.5) * W as f32,
                    (1.0 - (clip.y * invw * 0.5 + 0.5)) * H as f32,
                    clip.z * invw,
                    invw,
                ]
            };
            for tri in verts.chunks_exact(3) {
                let shade = tri[0].shade;
                let s = [
                    project(tri[0].pos),
                    project(tri[1].pos),
                    project(tri[2].pos),
                ];
                let uvw = [
                    [tri[0].uv[0] * s[0][3], tri[0].uv[1] * s[0][3]],
                    [tri[1].uv[0] * s[1][3], tri[1].uv[1] * s[1][3]],
                    [tri[2].uv[0] * s[2][3], tri[2].uv[1] * s[2][3]],
                ];
                for v in &s {
                    bx0 = bx0.min(v[0]);
                    by0 = by0.min(v[1]);
                    bx1 = bx1.max(v[0]);
                    by1 = by1.max(v[1]);
                }
                let (x0, y0, x1, y1, x2, y2) =
                    (s[0][0], s[0][1], s[1][0], s[1][1], s[2][0], s[2][1]);
                let area = (x1 - x0) * (y2 - y0) - (x2 - x0) * (y1 - y0);
                if area.abs() < 1e-6 {
                    continue;
                }
                let inv_area = 1.0 / area;
                let minx = x0.min(x1).min(x2).floor().max(0.0) as usize;
                let maxx = x0.max(x1).max(x2).ceil().min(W as f32 - 1.0) as usize;
                let miny = y0.min(y1).min(y2).floor().max(0.0) as usize;
                let maxy = y0.max(y1).max(y2).ceil().min(H as f32 - 1.0) as usize;
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
                        let idx = y * W + x;
                        if z >= zbuf[idx] {
                            continue;
                        }
                        let invw = w0 * s[0][3] + w1 * s[1][3] + w2 * s[2][3];
                        let u = (w0 * uvw[0][0] + w1 * uvw[1][0] + w2 * uvw[2][0]) / invw;
                        let v = (w0 * uvw[0][1] + w1 * uvw[1][1] + w2 * uvw[2][1]) / invw;
                        let lu = (u - au0) / (au1 - au0);
                        let lv = (v - av0) / (av1 - av0);
                        let sx = (lu * tw as f32).clamp(0.0, tw as f32 - 1.0) as u32;
                        let sy = (lv * th as f32).clamp(0.0, th as f32 - 1.0) as u32;
                        let texel = img.get_pixel(sx, sy).0;
                        if texel[3] < 128 {
                            continue;
                        }
                        zbuf[idx] = z;
                        let o = idx * 3;
                        color[o] = (texel[0] as f32 * shade) as u8;
                        color[o + 1] = (texel[1] as f32 * shade) as u8;
                        color[o + 2] = (texel[2] as f32 * shade) as u8;
                    }
                }
            }
            let name = if eat > 0.0 {
                let bite = if eat_bob >= 0.0 { "in" } else { "out" };
                let near = if eat_near > 0.0 { "_near" } else { "" };
                format!("{item:?}_eat{:.0}_bite_{bite}{near}", eat * 100.0).to_lowercase()
            } else {
                format!("{item:?}").to_lowercase()
            };
            let full = format!("/tmp/held_{name}.png");
            image::save_buffer(&full, &color, W as u32, H as u32, image::ColorType::Rgb8)
                .expect("save full");
            let pad = 24.0;
            let cx0 = (bx0 - pad).max(0.0) as usize;
            let cy0 = (by0 - pad).max(0.0) as usize;
            let cx1 = ((bx1 + pad).min(W as f32 - 1.0)) as usize;
            let cy1 = ((by1 + pad).min(H as f32 - 1.0)) as usize;
            let (cw, ch) = (cx1 - cx0 + 1, cy1 - cy0 + 1);
            let mut crop = vec![0u8; cw * 2 * ch * 2 * 3];
            for y in 0..ch * 2 {
                for x in 0..cw * 2 {
                    let srcp = ((cy0 + y / 2) * W + (cx0 + x / 2)) * 3;
                    let dst = (y * cw * 2 + x) * 3;
                    crop[dst..dst + 3].copy_from_slice(&color[srcp..srcp + 3]);
                }
            }
            let zoom = format!("/tmp/held_{name}_zoom.png");
            image::save_buffer(
                &zoom,
                &crop,
                (cw * 2) as u32,
                (ch * 2) as u32,
                image::ColorType::Rgb8,
            )
            .expect("save zoom");
            println!("wrote {full} + {zoom}  (roll={:.2})", item.held_pose().roll);
        }
    }
}
