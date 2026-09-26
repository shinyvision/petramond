//! Voxel game CLIENT session and scene state.
//!
//! `Game` is the client half of the client/server split, a coordinator
//! over owned subsystems: the locally-predicted player with its camera and
//! per-frame targeting ([`local_player::LocalPlayer`]), the replicated state
//! ([`replica_state::ReplicaState`]), the server link
//! ([`net_link::NetLink`]), prediction, the local hand, world presentation
//! effects ([`world_fx::WorldFx`]), the creative tools, and the app-facing
//! API. The SIMULATION — world, player sessions, entities, the
//! fixed-tick stage ladder — lives on the server, reached only through the
//! session's [`ServerHandle`](petramond::net::handle::ServerHandle).
//!
//! Input reaches the sim ONLY as [`petramond::net::protocol`]
//! messages: every frame the client translates its input + targeting into a
//! `PlayerUpdate` (+ one-shot `Action`s/menu actions queued on the
//! session's `NetLink`) and sends them to the server. The server
//! (`ServerGame`) runs on its OWN self-clocked thread behind a
//! `ServerHandle` — the handoff is std::sync::mpsc channels of message
//! VALUES (Arc payloads are refcount bumps); a remote join swaps TCP under
//! the identical messages.
//!
//! The server replies with ordered server→client MESSAGES (terrain payloads +
//! `TickUpdate`s): the client installs terrain into its own REPLICA world
//! (`ReplicaState::world` — rendering, collision, raycast, particles, door/chest
//! presentation all read it), entity/self state into the REPLICATED stores
//! (`ReplicaState::self_view`, `replicated.rs`). The client consumes ONLY those
//! messages: the tick's events (world-anchored + self one-shots) and the
//! menu-session view ride the `TickUpdate` (`ClientEvents`/
//! `ReplicaState::menu_view`); menus open server-side on the tick; tick-side
//! transform mutations come back as `SelfState::transform` corrections;
//! `tick_alpha` is a client-side clock over received updates
//! ([`tick::ReplicaClock`]). MOVEMENT-derived presentation (camera eye,
//! third-person pose) reads `LocalPlayer::player`. The LOCAL player is always
//! session 0 server-side.

pub mod ambient;
mod block_animation;
mod camera_rig;
pub mod body_pose;
mod client_mods;
mod client_presentation;
pub mod creative;
#[cfg(test)]
pub use petramond::menu as container;
mod bone_ease;
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
mod replica_state;
pub mod prediction;
pub mod presentation;
pub mod remote_players;
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
pub mod tools;
pub mod tick;
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

/// The client session: a thin coordinator over owned subsystems. Each
/// subsystem keeps its own state and invariants; `Game` passes explicit
/// borrows between them (the replica into the local player's physics and
/// targeting, the replica and the local player into prediction, ...).
pub struct Game {
    /// The shared background pool the replica streams on; presentation jobs
    /// (ghost meshes, library I/O) run there too.
    jobs: std::sync::Arc<petramond::worker::JobPool>,
    /// A refusal or failure to show the player once; the app takes it.
    pub notice: String,
    /// The creative building tools: world tools, schematic preview,
    /// library, share and ghosts.
    pub tools: tools::Tools,
    /// The link to the server: handle, frame batch, flow control and
    /// connection-loss latching. The client holds NO direct sim state.
    net: net_link::NetLink,
    /// Everything the server told this client: the replica world, the
    /// replicated entity stores, the self and menu views, and the per-batch
    /// inboxes (events, chat) drained once per frame.
    replica: replica_state::ReplicaState,
    /// The locally-simulated player: movement physics, the camera and its
    /// easing, per-frame targeting and the local input intents.
    local: local_player::LocalPlayer,
    /// Optional presentation-only client WASM modules. They read the replica
    /// and publish document state/images; they never share the server mod
    /// instances or simulation mutation seams.
    client_mods: petramond::modding::client::ClientModRuntime,
    /// Optimistic prediction ledger (request ids + undo snapshots + the
    /// presented-cell suppress set).
    pub prediction: prediction::PredictionLedger,
    /// The LOCAL hand's predicted one-shots (the ONLY source of the own
    /// hand animation — the server never echoes self-initiated one-shots)
    /// and the attack follow-through window.
    hand: local_hand::LocalHand,
    /// Client-side world presentation effects: particles, dig feedback,
    /// animated-block easing and the local body's bone easing.
    fx: world_fx::WorldFx,
}

impl Game {
    pub fn set_aspect(&mut self, aspect: f32) {
        self.local.cam.aspect = aspect;
    }

    /// The player's ear (eye) position, for the app layer's distance
    /// attenuation of positional mod sounds. Movement-derived → the client's
    /// predicted player.
    #[inline]
    pub fn listener_position(&self) -> petramond_math::world_pos::WorldPos {
        self.local.player.eye()
    }

    /// Current fixed-tick number, exposed for client-side presentation systems
    /// that schedule effects against game tick time without mutating the sim.
    /// The REPLICATED tick (latest `TickUpdate`), not a server-world read.
    #[inline]
    pub fn current_tick(&self) -> u64 {
        self.replica.entities.tick()
    }

    /// The OTHER connected players (id → name). Empty in singleplayer.
    #[cfg(test)]
    pub fn player_roster(&self) -> &std::collections::HashMap<petramond::player::PlayerId, String> {
        self.replica.entities.roster()
    }

    /// Request a survival/spectator toggle. The in-process listen player is
    /// intrinsically an operator and predicts immediately; TCP clients wait
    /// for the server-authoritative `SelfState::mode`, so an unprivileged
    /// client cannot enter spectator even briefly.
    pub fn toggle_player_mode(&mut self) {
        if !self.net.is_remote() {
            self.local.player.toggle_mode();
            self.replica.self_view.mode = self.local.player.mode();
        }
        self.net.queue(ClientToServer::Action(PlayerAction::ToggleMode));
    }

    #[cfg(test)]
    #[inline]
    pub fn player_mode(&self) -> PlayerMode {
        self.local.player.mode()
    }

    /// The CLIENT-owned active hotbar slot (what the number keys set and the
    /// next `PlayerUpdate` carries).
    #[cfg(test)]
    #[inline]
    pub fn active_hotbar(&self) -> u8 {
        self.local.player.inventory.active_slot()
    }

    /// Take the queued one-shot messages, for a test harness that services
    /// the server end synchronously (`Game::tick` sends them in play).
    #[cfg(test)]
    pub fn take_outbox_for_test(&mut self) -> Vec<ClientToServer> {
        self.net.take_outbox_for_test()
    }

    /// Apply replicated view refreshes a test harness built server-side,
    /// standing in for the next batch (`SelfState` + optional menu sync).
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

    /// Select hotbar `slot` (number key). Client-owned: the index rides the
    /// next `PlayerUpdate.hotbar_slot`; any hotbar change resets the R-key
    /// rotation cycle (clear-on-select). The replicated view mirrors it
    /// immediately — the server never echoes the index back (a lagged echo
    /// would yank a fast scroll).
    pub fn set_active_hotbar(&mut self, slot: u8) {
        self.local.player.inventory.set_active(slot);
        self.replica.self_view.inventory.set_active(slot);
        self.local.held_rotation.clear();
    }

    /// Cycle the held block's placement rotation (the R key). Client-owned:
    /// the armed-item check reads the REPLICATED inventory's selection; the
    /// raw counter rides the next `PlayerUpdate.held_rotation` and the
    /// session re-derives the armed item (see `HeldRotation::apply_wire`).
    pub fn toggle_held_block_rotation(&mut self) {
        let selected = self.replica.self_view.inventory.selected().map(|s| s.item);
        self.local.held_rotation.toggle(selected);
    }

    /// The in-progress eat progress for the LOCAL player (chew animation),
    /// read from the replicated self view.
    pub fn eating_progress(&self) -> Option<f32> {
        self.replica.self_view.eating
    }

    /// The held block's previewed placement state — the CLIENT's rotation
    /// cycle over the REPLICATED inventory's selected item (the render-path
    /// preview; the session keeps its own latched copy for the actual
    /// placement tick).
    #[inline]
    pub fn held_block_state(&self) -> HeldBlockState {
        self.local.held_rotation
            .held_block_state(self.replica.self_view.inventory.selected().map(|s| s.item))
    }

    // --- App-facing action methods. The pub surface `Game` exposed before the
    // client/server split stays intact, but these QUEUE
    // MESSAGES on the `NetLink` (flushed by `Game::tick`) instead of
    // touching server state. The menu
    // read model renders from the REPLICATED `MenuView` and the screen-open
    // calls are requests/acks (menus open server-side on the tick).

    /// App-side wake request (ESC / "Leave bed"), latched to the next tick.
    pub fn request_wake(&mut self) {
        self.net.queue(ClientToServer::Action(PlayerAction::Wake));
    }

    /// App-side respawn request (the death screen's button), latched to the
    /// next tick.
    pub fn request_respawn(&mut self) {
        self.net.queue(ClientToServer::Action(PlayerAction::Respawn));
    }

    /// Sleep fade progress in `[0, 1]` while the LOCAL player sleeps — the read
    /// model the presentation overlay darkens by, from the replicated self
    /// view. `None` while awake.
    pub fn sleep_progress01(&self) -> Option<f32> {
        self.replica.self_view.sleeping
    }

    /// `(sleeping, total)` across every connected player, the local one
    /// included — the server's headcount, since remote rows only reach this
    /// client for the players in its view. The sleep overlay shows
    /// "x/y players sleeping" from this when `total > 1`.
    pub fn sleeping_player_counts(&self) -> (usize, usize) {
        (
            usize::from(self.replica.entities.sleep_tally().sleeping),
            usize::from(self.replica.entities.sleep_tally().connected),
        )
    }

    /// The LOCAL player's health for the HUD hearts (replicated self view), or
    /// `None` when there is no survival bar to draw (a floating spectator).
    pub fn player_health(&self) -> Option<petramond_world::gui_state::HealthView> {
        if self.replica.self_view.mode != petramond::player::PlayerMode::Survival {
            return None;
        }
        Some(petramond_world::gui_state::HealthView {
            current: self.replica.self_view.health,
            max: petramond::player::MAX_HEALTH,
        })
    }

    /// The LOCAL player's active status effects for the HUD icon row, in
    /// application order (replicated self view). Empty for a spectator — the
    /// row hides with the hearts.
    pub fn player_effect_icons(&self) -> Vec<petramond_world::effect::Effect> {
        if self.replica.self_view.mode != petramond::player::PlayerMode::Survival {
            return Vec::new();
        }
        self.replica.self_view.effects.iter().map(|&(e, _)| e).collect()
    }

    /// Test injection: set the client's look target without a raycast, then
    /// run place prediction (the production path after `refresh_target`).
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
        // The shared consumer walk (main hand), so tests exercise the same
        // rungs, in the same order, that production clicks run.
        self.predict_main_hand_place(sneak)
    }

    /// Test injection: run the full local break prediction at `pos`.
    #[cfg(test)]
    pub fn predict_break_at_for_test(&mut self, pos: IVec3, block: petramond_world::block::Block) {
        self.apply_predicted_break(pos, block, Some(IVec3::Y));
    }

    /// Test injection: the whole TWO-PASS click verdict (main hand, then the
    /// off hand) against a synthetic look — what a production click computes
    /// in `build_outgoing_messages`.
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
