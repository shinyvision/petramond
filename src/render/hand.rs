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
//! is a complete clip-space transform. The mining punch (`swing` 0..1 sawtooth)
//! and one-shot place pop (`place_pop` 0..1) are folded into that transform here.

use glam::{Mat4, Quat, Vec3};

use super::block_model::{push_cube_solid_lit, push_cube_textured_lit};
#[cfg(test)]
use super::lighting;
use super::{HeldItemFrame, HeldItemView};
use crate::atlas::Tile;
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

/// Mining-punch swings per second. Drives the looping hand swing phase while the
/// sim reports active mining.
const HAND_SWING_HZ: f32 = 4.2;

/// Duration of the one-shot place jab, in seconds.
const PLACE_ANIM_SECS: f32 = 0.25;

#[derive(Copy, Clone, Debug)]
pub(super) struct HeldItemAnimator {
    swing_t: f32,
    swing_finishing: bool,
    place_anim_t: f32,
}

impl Default for HeldItemAnimator {
    fn default() -> Self {
        Self {
            swing_t: 0.0,
            swing_finishing: false,
            place_anim_t: 1.0,
        }
    }
}

impl HeldItemAnimator {
    pub fn update(&mut self, frame: HeldItemFrame) -> HeldItemView {
        let dt = frame.dt.max(0.0);
        if frame.mining {
            self.swing_finishing = false;
            self.swing_t = (self.swing_t + dt * HAND_SWING_HZ).fract();
        } else {
            self.swing_finishing |= frame.broke_block;
        }

        if !frame.mining && (self.swing_finishing || self.swing_t > 0.0) {
            let next = self.swing_t + dt * HAND_SWING_HZ;
            if next >= 1.0 {
                self.swing_t = 0.0;
                self.swing_finishing = false;
            } else {
                self.swing_t = next;
            }
        }

        if frame.placed {
            self.place_anim_t = 0.0;
        } else {
            self.place_anim_t = (self.place_anim_t + dt / PLACE_ANIM_SECS).min(1.0);
        }

        HeldItemView {
            item: frame.item,
            swing: self.swing_t,
            place_pop: (1.0 - self.place_anim_t).clamp(0.0, 1.0),
        }
    }
}

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
    build_hand_lit(view, aspect, lighting::FULL_SKYLIGHT, verts, indices)
}

pub(super) fn build_hand_lit(
    view: &HeldItemView,
    aspect: f32,
    skylight: u8,
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
                skylight,
            );
            Mat4::from_scale(Vec3::new(4.0, 12.0, 4.0))
        }
        Some(item) => match item.render_kind() {
            ItemRenderKind::BlockCube(block) => {
                // Held block: a corner toward the camera (three-quarter view).
                push_cube_textured_lit(
                    verts,
                    indices,
                    block.tiles(),
                    Vec3::new(-0.5, -0.5, -0.5),
                    1.0,
                    skylight,
                );
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
        },
    };

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
    let ItemRenderKind::Sprite(tile) = view.item?.render_kind() else {
        return None;
    };
    // Three-quarter tilt: yaw + a small pitch so the front/back faces and the
    // stepped side walls all read. The extruded mesh is a unit, origin-centred
    // slab, so this scale sizes it like a held item — a touch larger than a held
    // block so the flat sprite reads clearly in hand and its extrusion shows.
    let s = 0.85;
    let base_model = Mat4::from_scale_rotation_translation(
        Vec3::splat(s),
        Quat::from_rotation_y(0.7) * Quat::from_rotation_x(-0.25) * Quat::from_rotation_z(-0.15),
        Vec3::ZERO,
    );
    Some((
        tile,
        hand_view_proj(aspect) * held_item_placement(view, aspect) * base_model,
    ))
}

/// Fixed first-person perspective for the hand (independent of the world camera).
fn hand_view_proj(aspect: f32) -> Mat4 {
    let proj = Mat4::perspective_rh(HAND_FOV_Y, aspect.max(0.0001), 0.01, 10.0);
    // Camera at origin looking down -Z; the model sits a short distance ahead.
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

fn vanilla_arm_swing_translation(swing: f32) -> Vec3 {
    let s = swing.clamp(0.0, 1.0);
    let root = s.sqrt();
    let f2 = -0.3 * (root * std::f32::consts::PI).sin();
    let f3 = 0.4 * (root * std::f32::consts::TAU).sin();
    let f4 = -0.4 * (s * std::f32::consts::PI).sin();
    Vec3::new(f2 + 0.64000005, f3 - 0.6, f4 - 0.71999997)
}

fn vanilla_player_arm_pose(swing: f32) -> Mat4 {
    let s = swing.clamp(0.0, 1.0);
    let root = s.sqrt();
    let f5 = (s * s * std::f32::consts::PI).sin();
    let f6 = (root * std::f32::consts::PI).sin();

    Mat4::from_translation(vanilla_arm_swing_translation(s))
        * Mat4::from_rotation_y(radians(45.0))
        * Mat4::from_rotation_y(radians(f6 * 70.0))
        * Mat4::from_rotation_z(radians(f5 * -20.0))
        * Mat4::from_translation(Vec3::new(-1.0, 3.6, 3.5))
        * Mat4::from_rotation_z(radians(120.0))
        * Mat4::from_rotation_x(radians(200.0))
        * Mat4::from_rotation_y(radians(-135.0))
        * Mat4::from_translation(Vec3::new(5.6, 0.0, 0.0))
}

fn bare_arm_placement(view: &HeldItemView, aspect: f32) -> Mat4 {
    let aspect = aspect.max(0.0001);
    let anchor = view_pos_from_ndc(
        VANILLA_ARM_ANCHOR_NDC_X,
        VANILLA_ARM_ANCHOR_NDC_Y,
        BARE_ARM_DEPTH,
        aspect,
    );
    let mut placement = Mat4::from_translation(anchor)
        * Mat4::from_scale(Vec3::splat(VANILLA_ARM_SCALE))
        * vanilla_player_arm_pose(view.swing);

    if view.place_pop > 0.0 {
        let p = view.place_pop.clamp(0.0, 1.0);
        let pop = p * p * (3.0 - 2.0 * p);
        placement = Mat4::from_translation(Vec3::new(0.0, -0.05 * pop, -0.28 * pop))
            * Mat4::from_rotation_x(-0.25 * pop)
            * placement;
    }

    placement
}

/// Place held item models in the lower-right and apply swing / place-pop animation.
fn held_item_placement(view: &HeldItemView, aspect: f32) -> Mat4 {
    let aspect = aspect.max(0.0001);
    let rest = view_pos_from_ndc(REST_NDC_X, REST_NDC_Y, HAND_DEPTH, aspect);
    let mut pos = rest;
    let mut rot = Quat::IDENTITY;

    if view.place_pop > 0.0 {
        let p = view.place_pop.clamp(0.0, 1.0);
        let pop = p * p * (3.0 - 2.0 * p);
        pos += Vec3::new(0.0, -0.05 * pop, -0.28 * pop);
        rot = Quat::from_rotation_x(-0.25 * pop) * rot;
    }

    if view.swing > 0.0 {
        let s = view.swing.clamp(0.0, 1.0);
        let root_sin = (std::f32::consts::PI * s.sqrt()).sin();
        let swing_sin = (std::f32::consts::PI * s).sin();
        let swing_sq_sin = (std::f32::consts::PI * s * s).sin();

        pos += Vec3::new(
            -0.30 * root_sin,
            0.40 * (std::f32::consts::TAU * s.sqrt()).sin(),
            -0.40 * swing_sin,
        );
        let attack = Quat::from_rotation_y(radians(45.0 + swing_sq_sin * -20.0))
            * Quat::from_rotation_z(radians(root_sin * -20.0))
            * Quat::from_rotation_x(radians(root_sin * -80.0))
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
    fn animator_completes_active_swing_when_mining_stops() {
        let mut anim = HeldItemAnimator {
            swing_t: 0.5,
            ..HeldItemAnimator::default()
        };
        let view = anim.update(HeldItemFrame {
            item: None,
            mining: false,
            broke_block: false,
            placed: false,
            dt: 1.0 / 60.0,
        });
        assert!(
            view.swing > 0.5,
            "stopping mining should finish the swing forward, not rewind it"
        );

        let settled = anim.update(HeldItemFrame {
            item: None,
            mining: false,
            broke_block: false,
            placed: false,
            dt: 0.5 / HAND_SWING_HZ,
        });
        assert_eq!(settled.swing, 0.0);
    }

    #[test]
    fn animator_plays_one_swing_for_instant_break_from_rest() {
        let mut anim = HeldItemAnimator::default();

        let started = anim.update(HeldItemFrame {
            item: None,
            mining: false,
            broke_block: true,
            placed: false,
            dt: 0.0,
        });
        assert_eq!(
            started.swing, 0.0,
            "zero-dt break event can begin at the rest pose"
        );

        let moving = anim.update(HeldItemFrame {
            item: None,
            mining: false,
            broke_block: false,
            placed: false,
            dt: 1.0 / 60.0,
        });
        assert!(
            moving.swing > 0.0,
            "instant block break should keep animating after the break frame"
        );

        let settled = anim.update(HeldItemFrame {
            item: None,
            mining: false,
            broke_block: false,
            placed: false,
            dt: 1.0 / HAND_SWING_HZ,
        });
        assert_eq!(settled.swing, 0.0);
    }

    #[test]
    fn animator_turns_place_event_into_one_shot_pop() {
        let mut anim = HeldItemAnimator::default();
        let placed = anim.update(HeldItemFrame {
            item: Some(ItemType::Dirt),
            mining: false,
            broke_block: false,
            placed: true,
            dt: 1.0 / 60.0,
        });
        assert_eq!(placed.place_pop, 1.0);

        let settled = anim.update(HeldItemFrame {
            item: Some(ItemType::Dirt),
            mining: false,
            broke_block: false,
            placed: false,
            dt: PLACE_ANIM_SECS,
        });
        assert_eq!(settled.place_pop, 0.0);
    }

    #[test]
    fn bare_hand_builds_solid_cuboid() {
        let view = HeldItemView {
            item: None,
            swing: 0.0,
            place_pop: 0.0,
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
            swing: 0.0,
            place_pop: 0.0,
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
            swing: 0.0,
            place_pop: 0.0,
        };
        let (mut v, mut i) = (Vec::new(), Vec::new());

        build_hand_lit(&view, 16.0 / 9.0, 9, &mut v, &mut i);

        assert!(!v.is_empty());
        for vert in &v {
            assert_eq!((vert.packed >> 23) & 0x3F, 9);
        }
    }

    #[test]
    fn held_sprite_emits_no_model3d_geometry() {
        // Sprite items are drawn by the renderer via the item3d (extruded)
        // pipeline, NOT the model3d hand pass, so build_hand emits nothing.
        let view = HeldItemView {
            item: Some(ItemType::Poppy),
            swing: 0.0,
            place_pop: 0.0,
        };
        let (mut v, mut i) = (Vec::new(), Vec::new());
        build_hand(&view, 16.0 / 9.0, &mut v, &mut i);
        assert!(v.is_empty(), "sprite hand emits no model3d verts");
        assert!(i.is_empty(), "sprite hand emits no model3d indices");
    }

    #[test]
    fn held_sprite_reports_tile_and_mvp() {
        // held_sprite drives the extruded item3d draw; it must report the sprite
        // tile (and a finite MVP) for a sprite item and None otherwise.
        let poppy = HeldItemView {
            item: Some(ItemType::Poppy),
            swing: 0.0,
            place_pop: 0.0,
        };
        let (tile, mvp) = held_sprite(&poppy, 16.0 / 9.0).expect("sprite reports a tile");
        assert_eq!(tile as u8, crate::atlas::Tile::Poppy as u8);
        assert!(mvp.to_cols_array().iter().all(|f| f.is_finite()));
        // Bare hand + held block return None (they go through build_hand).
        let bare = HeldItemView {
            item: None,
            ..poppy
        };
        let block = HeldItemView {
            item: Some(ItemType::Stone),
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
            swing: 0.0,
            place_pop: 0.0,
        };
        let bare = HeldItemView {
            item: None,
            swing: 0.0,
            place_pop: 0.0,
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
            swing: 0.0,
            place_pop: 0.0,
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
            swing: 0.0,
            place_pop: 0.0,
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
            swing: 0.0,
            place_pop: 0.0,
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
    fn vanilla_player_arm_pose_uses_official_swing_offsets() {
        let s = 0.5f32;
        let root = s.sqrt();
        let expected = Vec3::new(
            -0.3 * (root * std::f32::consts::PI).sin() + 0.64000005,
            0.4 * (root * std::f32::consts::TAU).sin() - 0.6,
            -0.4 * (s * std::f32::consts::PI).sin() - 0.71999997,
        );
        let actual = vanilla_arm_swing_translation(s);
        assert!(
            (actual - expected).length() < 1e-6,
            "vanilla arm swing translation must match the official constants"
        );
        assert!(
            expected.x < 0.64000005,
            "swing moves the right arm toward center"
        );
        assert!(expected.z < -0.71999997, "swing thrusts the arm forward");
    }

    #[test]
    fn swing_and_pop_change_the_mvp() {
        let rest = HeldItemView {
            item: Some(ItemType::Stone),
            swing: 0.0,
            place_pop: 0.0,
        };
        let mid_swing = HeldItemView { swing: 0.5, ..rest };
        let popping = HeldItemView {
            place_pop: 0.5,
            ..rest
        };
        let (mut v, mut i) = (Vec::new(), Vec::new());
        let a = build_hand(&rest, 1.5, &mut v, &mut i);
        let b = build_hand(&mid_swing, 1.5, &mut v, &mut i);
        let c = build_hand(&popping, 1.5, &mut v, &mut i);
        assert_ne!(a, b, "mid-swing must move the hand");
        assert_ne!(a, c, "place pop must move the hand");
    }
}
