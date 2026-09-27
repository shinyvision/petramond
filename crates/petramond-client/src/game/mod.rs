//! Voxel game CLIENT session and scene state.
//!
//! `Game` is the client half of the client/server split. It coordinates the locally-predicted
//! player with its camera and targeting ([`local_player::LocalPlayer`]), the replicated state
//! ([`replica_state::ReplicaState`]), the server link ([`net_link::NetLink`]), prediction, the
//! local hand, world effects ([`world_fx::WorldFx`]), the creative tools and the app-facing API.
//! The simulation lives on the server, reached only through the session's
//! [`ServerHandle`](petramond::net::handle::ServerHandle).
//!
//! Input reaches the sim only as [`petramond::net::protocol`] messages. Each frame the client
//! turns input and targeting into a `PlayerUpdate`, plus one-shot `Action`s and menu actions
//! queued on the `NetLink`. `ServerGame` runs on its own self-clocked thread, fed by mpsc channels
//! of message values. A remote join swaps in TCP under the same messages.
//!
//! The server replies with ordered messages: terrain payloads and `TickUpdate`s. Terrain goes into
//! the client's replica world (`ReplicaState::world`), which rendering, collision, raycast and
//! presentation read. Entity and self state go into the replicated stores
//! (`ReplicaState::self_view`, `replicated.rs`). Tick events and the menu-session view ride the
//! `TickUpdate` (`ClientEvents`, `ReplicaState::menu_view`). Tick-side transform changes come back
//! as `SelfState::transform` corrections. `tick_alpha` is a client-side clock over received
//! updates ([`tick::ReplicaClock`]). The local player is always session 0 on the server.

pub mod ambient;
mod block_animation;
pub mod body_pose;
mod camera_rig;
mod capture;
mod client_mods;
mod presented_entities;
mod view_subject;
pub use view_subject::SubjectHands;
mod client_presentation;
pub mod creative;
#[cfg(test)]
pub use petramond::menu as container;
mod bone_ease;
mod captured_view;
mod dig_feedback;
pub mod environment;
mod first_person;
mod frame;
mod ghosts;
mod local_hand;
mod local_player;
mod menu_actions;
mod menu_prediction;
mod net_link;
pub mod prediction;
pub mod presentation;
mod presenting;
pub mod remote_players;
mod replica_state;
pub mod replicated;
pub mod schematic_library;
pub mod schematic_preview;
pub mod schematics;
pub mod section_cache;
pub mod selection_tool;
pub mod session;
mod session_control;
mod speed_fov;
mod terrain_render;
mod third_person;
pub mod tick;
pub mod tools;
mod view_bob;
mod world_fx;
mod world_prediction;
pub mod world_tool;

use petramond::net::protocol::{ClientToServer, PlayerAction};
#[cfg(test)]
use petramond::player::{PlayerMode, RaycastHit};
#[cfg(test)]
use petramond_math::math::IVec3;
use petramond_world::block_state::HeldBlockState;

pub use environment::GameEnvironment;
pub use frame::{render_bone_offsets, render_held_pose};
pub use menu_actions::MenuReadModel;
pub use tick::{
    GameEvents, GameInput, MobSoundEvent, MovementInput, SpatialSoundCommand, WorldEvent,
};

pub struct Game {
    jobs: std::sync::Arc<petramond::worker::JobPool>,
    pub notice: String,
    pub tools: tools::Tools,
    net: net_link::NetLink,
    replica: replica_state::ReplicaState,
    local: local_player::LocalPlayer,
    client_mods: petramond::modding::client::ClientModRuntime,
    pub prediction: prediction::PredictionLedger,
    hand: local_hand::LocalHand,
    fx: world_fx::WorldFx,
    presented_entities_cache: Vec<mod_api::ClientEntityData>,
    last_anchor_feet: Option<(mod_api::EntityRef, petramond_math::world_pos::WorldPos)>,
    anchor_missing: bool,
    presenting: presenting::Presenting,
    world_capture: capture::WorldCapture,
}

impl Game {
    pub fn set_aspect(&mut self, aspect: f32) {
        self.local.cam.aspect = aspect;
        if let Some(boom) = self.local.third_person.cam.as_mut() {
            boom.aspect = aspect;
        }
    }

    #[inline]
    pub fn listener_position(&self) -> petramond_math::world_pos::WorldPos {
        self.render_camera().pos
    }

    #[inline]
    pub fn current_tick(&self) -> u64 {
        self.replica.entities.tick()
    }

    #[cfg(test)]
    pub fn player_roster(&self) -> &std::collections::HashMap<petramond::player::PlayerId, String> {
        self.replica.entities.roster()
    }

    pub fn toggle_player_mode(&mut self) {
        if !self.net.is_remote() {
            self.local.player.toggle_mode();
            self.replica.self_view.mode = self.local.player.mode();
        }
        self.net
            .queue(ClientToServer::Action(PlayerAction::ToggleMode));
    }

    #[cfg(test)]
    #[inline]
    pub fn player_mode(&self) -> PlayerMode {
        self.local.player.mode()
    }

    #[cfg(test)]
    #[inline]
    pub fn active_hotbar(&self) -> u8 {
        self.local.player.inventory.active_slot()
    }

    #[cfg(test)]
    pub fn take_outbox_for_test(&mut self) -> Vec<ClientToServer> {
        self.net.take_outbox_for_test()
    }

    #[cfg(test)]
    pub fn apply_views_for_test(
        &mut self,
        state: &petramond::net::protocol::SelfState,
        sync: Option<petramond::net::protocol::MenuSyncMsg>,
    ) {
        self.replica.self_view.apply(state, true);
        if let Some(sync) = sync {
            self.replica.menu_view.apply(sync);
        }
    }

    pub fn set_active_hotbar(&mut self, slot: u8) {
        self.local.player.inventory.set_active(slot);
        self.replica.self_view.inventory.set_active(slot);
        self.local.held_rotation.clear();
    }

    pub fn toggle_held_block_rotation(&mut self) {
        let selected = self.replica.self_view.inventory.selected().map(|s| s.item);
        self.local.held_rotation.toggle(selected);
    }

    pub fn eating_progress(&self) -> Option<f32> {
        self.replica.self_view.eating
    }

    #[inline]
    pub fn held_block_state(&self) -> HeldBlockState {
        self.local
            .held_rotation
            .held_block_state(self.replica.self_view.inventory.selected().map(|s| s.item))
    }

    pub fn request_wake(&mut self) {
        self.net.queue(ClientToServer::Action(PlayerAction::Wake));
    }

    pub fn request_respawn(&mut self) {
        self.net
            .queue(ClientToServer::Action(PlayerAction::Respawn));
    }

    pub fn sleep_progress01(&self) -> Option<f32> {
        self.replica.self_view.sleeping
    }

    pub fn sleeping_player_counts(&self) -> (usize, usize) {
        (
            usize::from(self.replica.entities.sleep_tally().sleeping),
            usize::from(self.replica.entities.sleep_tally().connected),
        )
    }

    pub fn player_health(&self) -> Option<petramond_world::gui_state::HealthView> {
        self.replica.self_view.health_view()
    }

    pub fn player_effect_icons(&self) -> Vec<petramond_world::effect::Effect> {
        self.replica.self_view.effect_icons()
    }

    #[cfg(test)]
    pub fn predict_place_at_for_test(
        &mut self,
        block: IVec3,
        normal: IVec3,
        sneak: bool,
    ) -> crate::game::tick::PlacePrediction {
        self.local.look = Some(RaycastHit {
            block,
            normal,
            spot: petramond_math::math::Vec3::splat(0.5),
            outline: petramond_world::selection::SelectionShape::full_block(block),
        });
        self.predict_main_hand_place(sneak)
    }

    #[cfg(test)]
    pub fn predict_break_at_for_test(&mut self, pos: IVec3, block: petramond_world::block::Block) {
        self.apply_predicted_break(pos, block, Some(IVec3::Y));
    }

    #[cfg(test)]
    pub fn predict_click_verdict_at_for_test(
        &mut self,
        block: IVec3,
        normal: IVec3,
        sneak: bool,
    ) -> crate::game::tick::ClickVerdict {
        self.local.look = Some(RaycastHit {
            block,
            normal,
            spot: petramond_math::math::Vec3::splat(0.5),
            outline: petramond_world::selection::SelectionShape::full_block(block),
        });
        let mut input = crate::game::tick::GameInput::default();
        input.movement.sneak = sneak;
        self.predict_click_verdict(&input, None)
    }
}

#[cfg(test)]
pub(crate) mod tests;
