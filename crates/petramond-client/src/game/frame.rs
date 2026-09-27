//! Neutral app/client frame state read from [`Game`].
//!
//! This is the game-side boundary for App update/render: camera, environment,
//! and target/held-item state. It intentionally does not contain renderer DTOs
//! or terrain upload handles.

use petramond_render::camera::Camera;
use petramond_world::block_state::HeldBlockState;
use petramond_world::item::ItemType;
use petramond_world::selection::SelectionShape;

use super::{Game, GameEnvironment};
use crate::animation::BoneOffset;

pub struct ClientFrame<'a> {
    /// The frame's ONE camera ([`Game::render_camera`]): what the view draws
    /// from and where the listener hears from.
    pub camera: &'a Camera,
    pub environment: GameEnvironment,
    pub selection: Option<SelectionShape>,
    pub held_item: ClientHeldItem,
    /// The OFF-hand (left) item's frame state. `item == None` = empty slot —
    /// no left hand renders. Mining/rotation stay main-hand-only; an off-hand
    /// eat carries its progress here instead of on `held_item`.
    pub off_hand_item: ClientHeldItem,
    /// The local body's resolved animator claims for both rigs — predicted
    /// per key by a client mod claiming it, replicated otherwise.
    pub animator: petramond::player::AnimatorClaims,
}

impl ClientFrame<'_> {
    /// The spatial-audio listener: the ears ride the presented camera, so a
    /// claimed camera hears from where it looks.
    pub fn listener(&self) -> petramond_audio::SpatialListener {
        petramond_audio::SpatialListener {
            pos: self.camera.pos,
            right: self.camera.right(),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ClientHeldItem {
    pub item: Option<ItemType>,
    /// What the hand DRAWS in place of `item`'s own art, when a mod claims
    /// one (predicted locally, replicated otherwise); `None` = `item`.
    pub display: Option<ItemType>,
    pub variant: petramond_world::item::VariantId,
    pub block_state: HeldBlockState,
    pub mining: bool,
    /// A food item is mid-eat (held secondary button): the eat's progress in
    /// `[0, 1)` — the animation carries the food deeper toward the mouth as it
    /// advances. `None` on ordinary frames.
    pub eating: Option<f32>,
    /// This hand's claimed held pose — the target the
    /// held item's easing glides toward. PREDICTED by a client mod when one poses
    /// hands here, replicated otherwise. `None` = the item's authored hold.
    pub pose_target: Option<mod_api::HeldPose>,
}

/// Carry a claimed held pose across the ABI → RENDERER boundary.
///
/// The two structs are deliberately not one type: the mod ABI's is a wire
/// value with its own compatibility rules, the renderer's is a
/// `DisplayTransform` pair it composes with the authored hold. They agree on
/// the vocabulary — Blockbench display units, rotation degrees XYZ,
/// translation in 1/16-block pixels — so the carry is a field copy, and the
/// scale/pivot channels a mod cannot set stay at their authored identity.
pub fn render_held_pose(pose: mod_api::HeldPose) -> petramond_render::HeldPose {
    let view = |p: mod_api::HeldPoseData| petramond_world::block_model::DisplayTransform {
        rotation: p.rotation,
        translation: p.translation,
        ..Default::default()
    };
    petramond_render::HeldPose {
        first_person: view(pose.first_person),
        third_person: view(pose.third_person),
    }
}

/// Carry resolved bone poses across the SIM → ANIMATION boundary the way
/// [`render_held_pose`] carries a held pose.
///
/// Both sides address a bone by its index in the player rig — the mod ABI's
/// names were resolved once, at the host call — so this is a field copy. An id
/// this build's rig has no bone for is DROPPED, not an error: an offset aimed
/// at a bone that is not there is a disabled pack, and the frame still draws.
///
/// Appends into `out` rather than returning, so each body reuses its own
/// target buffer frame after frame.
pub fn render_bone_offsets(poses: &[petramond::player::BonePose], out: &mut Vec<BoneOffset>) {
    let bones = petramond::player::model::player_model().bones().len();
    out.extend(
        poses
            .iter()
            .filter(|p| (p.bone as usize) < bones)
            .map(|p| BoneOffset {
                bone: p.bone as usize,
                rotation: p.rotation,
                translation: p.translation,
                hold: p.hold,
            }),
    );
}

impl Game {
    /// Coherent neutral app-facing state for update/render after the game
    /// tick. Held-item/mining/eating state reads the REPLICATED self view,
    /// never the server session.
    pub fn client_frame(&self, now: f64) -> ClientFrame<'_> {
        let view = &self.replica.self_view;
        let mining = view.mining.is_some();
        // The one in-progress eat belongs to a HAND: its progress animates the
        // hand that is carrying the food, and only that one.
        let eating = self.eating_progress();
        let (eat_main, eat_off) = if view.eating_off_hand {
            (None, eating)
        } else {
            (eating, None)
        };
        // A client mod running the same rule as its server half answers a
        // round trip sooner, so it owns the hands it poses; hands no client
        // mod poses keep the replicated answer.
        let (pose_main, pose_off) = self
            .client_mods
            .local_held_poses((view.held_pose_main, view.held_pose_off));
        let [display_main, display_off] = self.client_mods.local_held_displays(view.held_display);
        let animator = self.client_mods.local_animator(&view.animator);
        let mut environment = self.environment(now);
        environment.shader_params = self.presented_shader_params(&environment.shader_params);
        ClientFrame {
            // Sim consumers keep reading `self.local.cam` directly.
            camera: self.render_camera(),
            environment,
            selection: self.local.look.map(|h| h.outline),
            held_item: ClientHeldItem {
                item: view.inventory.selected().map(|s| s.item),
                display: display_main,
                variant: view
                    .inventory
                    .selected()
                    .map(|s| s.variant)
                    .unwrap_or_default(),
                block_state: self.held_block_state(),
                mining,
                eating: eat_main,
                pose_target: pose_main,
            },
            off_hand_item: ClientHeldItem {
                item: view.inventory.off_hand().map(|s| s.item),
                display: display_off,
                variant: view
                    .inventory
                    .off_hand()
                    .map(|s| s.variant)
                    .unwrap_or_default(),
                // The R-rotation preview arms on the SELECTED item only.
                block_state: Default::default(),
                mining: false,
                eating: eat_off,
                pose_target: pose_off,
            },
            animator,
        }
    }
}
