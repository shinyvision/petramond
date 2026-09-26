//! Voxel game CLIENT session and scene state.
//!
//! `Game` is the client half of the client/server split: the
//! camera, the locally-predicted player (`Game::player` — movement physics
//! and the camera source), per-frame targeting (`Game::look`/
//! `Game::targeted_mob`), particles, transient animation state, and the
//! app-facing API. The SIMULATION — world, player sessions, entities, the
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
//! (`Game::replica` — rendering, collision, raycast, particles, door/chest
//! presentation all read it), entity/self state into the REPLICATED stores
//! (`Game::self_view`, `replicated.rs`). The client consumes ONLY those
//! messages: the tick's events (world-anchored + self one-shots) and the
//! menu-session view ride the `TickUpdate` (`ClientEvents`/
//! `Game::menu_view`); menus open server-side on the tick; tick-side
//! transform mutations come back as `SelfState::transform` corrections;
//! `tick_alpha` is a client-side clock over received updates
//! ([`tick::ReplicaClock`]). MOVEMENT-derived presentation (camera eye,
//! third-person pose) reads `self.player`. The LOCAL player is always
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
mod world_prediction;
pub mod world_tool;

use std::collections::HashMap;

use crate::particle::ParticleSystem;
use petramond::net::protocol::{ChatLine, ClientToServer, PlayerAction, SelfTransform};
#[cfg(test)]
use petramond::player::PlayerMode;
use petramond::player::{Player, RaycastHit};
use petramond::world::ReplicaWorld;
#[cfg(test)]
use petramond_math::math::IVec3;
use petramond_render::camera::Camera;
use petramond_world::block_state::HeldBlockState;
use petramond_world::world::placement::HeldRotation;
use petramond_worldgen::density::surface::SurfaceDensitySystem;

pub use environment::GameEnvironment;
pub use frame::{render_bone_offsets, render_held_pose};
pub use menu_actions::MenuReadModel;
pub use tick::{
    GameEvents, GameInput, MobSoundEvent, MovementInput, SpatialSoundCommand, WorldEvent,
};

pub struct Game {
    /// The shared background pool the replica streams on; presentation jobs
    /// (ghost meshes, library I/O) run there too.
    jobs: std::sync::Arc<petramond::worker::JobPool>,
    /// A refusal or failure to show the player once; the app takes it.
    pub notice: String,
    /// The creative building tools: world tools, schematic preview,
    /// library, share and ghosts.
    pub tools: tools::Tools,
    /// Creative double-jump flight toggling.
    flight_toggle: creative::FlightToggle,
    /// Creative instant-break repeat pacing while the button is held.
    break_repeat: creative::BreakRepeat,
    cam: Camera,
    /// The client's LOCALLY-SIMULATED player: movement physics runs on this
    /// copy every frame and the camera mirrors its eye. Its transform is sent
    /// to the server in each frame's `PlayerUpdate` (trusted verbatim for the
    /// local session); server-side transform mutations (teleports, knockback)
    /// are adopted back after the fixed ticks. Its INVENTORY CONTENTS are a
    /// stale clone — only the active-slot index is meaningful client-side; the
    /// authoritative inventory lives on the session.
    player: Player,
    /// The client's per-frame raycast target: presentation (selection outline,
    /// mining dust) + the `PlayerUpdate.target` message source. `None` when a
    /// mob is the closer target.
    look: Option<RaycastHit>,
    /// The USE-click target this frame: equal to [`look`](Self::look) unless
    /// the held item declares a water-stopping use ray (`use_ray: water` in
    /// `items.json`), in which case the first water cell in reach can be the
    /// target. Rides `UseClick.target` only — selection outline, mining, and
    /// the look latch keep the normal water-transparent ray.
    use_look: Option<RaycastHit>,
    /// The mob under the crosshair this frame (STABLE replicated id), nearer
    /// than any block. Refreshed per frame from the replicated rows; the click
    /// actions carry it on the wire.
    targeted_mob: Option<u64>,
    /// The remote PLAYER under the crosshair this frame (`PlayerId` byte),
    /// nearer than any block or mob — the PvP attack target. At most one of
    /// `targeted_mob`/`targeted_player` is set; `AttackClick` carries it.
    targeted_player: Option<u8>,
    /// The client-owned R-key placement-rotation cycle; its raw counter rides
    /// `PlayerUpdate.held_rotation` (the session keeps its own latched copy).
    held_rotation: HeldRotation,
    /// The first-person camera's presentation easing (step glide, sneak dip,
    /// walking sway, pillow eye, speed FOV).
    camera_rig: camera_rig::CameraRig,
    /// Third-person view state (boom camera + body pose). `cam` above stays the
    /// authoritative first-person eye for every presentation consumer; see
    /// `third_person.rs`.
    third_person: third_person::ThirdPerson,
    /// The link to the server: handle, frame batch, flow control and
    /// connection-loss latching. The client holds NO direct sim state.
    net: net_link::NetLink,
    /// The transform of the last `PlayerUpdate` this client SENT. A
    /// `SelfState::transform` correction adopts only the fields that differ
    /// from it: fields equal to what we last claimed are just the server
    /// echoing us, and the local (possibly newer) value wins.
    last_sent_transform: Option<SelfTransform>,
    /// Replica sections installed during the current message drain. Their
    /// overlapping mesh invalidations are applied once after the batch.
    remote_section_installs: Vec<petramond_world::chunk::SectionPos>,
    /// Chat lines received from the server and not yet adopted by the app's
    /// client-side chat history.
    pending_chat_lines: Vec<ChatLine>,
    /// The client's REPLICA world: installed from the
    /// server's terrain payloads + deltas, it owns light + meshes for the
    /// renderer and answers every client-side world read — collision, raycast,
    /// particles, door/chest presentation, environment sampling.
    replica: ReplicaWorld,
    /// Optional presentation-only client WASM modules. They read the replica
    /// and publish document state/images; they never share the server mod
    /// instances or simulation mutation seams.
    client_mods: petramond::modding::client::ClientModRuntime,
    /// The replicated ENTITY state: mob / item / remote-player stores, the
    /// own mount, the staged interpolation window over them, and the
    /// per-batch session facts (tick, sleep headcount, roster, own id).
    entities: replicated::EntityReplica,
    /// The client-side mirror of the local player's replicated `SelfState`:
    /// the HUD/hand/overlay read model (health, effects, inventory, mining,
    /// eating, sleeping).
    self_view: replicated::SelfView,
    /// The client's replicated MENU-session view (`MenuSyncMsg`, on-change),
    /// including disposable P1 menu predictions until their outcomes arrive:
    /// the exclusive source `menu_read_model` renders container screens from.
    menu_view: replicated::MenuView,
    /// Immutable enabled player-crafting catalog agreed at join. Remote
    /// clients use the server's name-addressed rows, never local pack files.
    crafting: petramond_world::crafting::CraftingCatalog,
    /// This frame's replicated tick events (world-anchored + self one-shots +
    /// sound queues), buffered by `apply_tick_update` and drained once per
    /// `Game::tick` into `GameEvents`.
    pending_events: tick::ClientEvents,
    /// Optimistic prediction ledger (request ids + undo snapshots + the
    /// presented-cell suppress set).
    pub prediction: prediction::PredictionLedger,
    /// Local mining timer for crack overlay + `BreakFinished` (P2).
    local_mining: petramond_world::mining::MiningState,
    /// The movement `Input` this frame's local physics consumed
    /// (`tick_player`) — reused verbatim by `build_player_update` so the wire
    /// intent can never drift from what the prediction simulated.
    predicted_input: petramond::player::Input,
    /// The gameplay-gated use (interact) button intent this frame — the
    /// client twin of the server's `sess.using()`; feeds the client actor
    /// snapshot's `use_held` for mod predictors.
    intent_use_held: bool,
    /// The local body's eased bone offsets — the third-person twin of the
    /// held item's pose easing, at the same rate. Holds the eased
    /// value between frames; the presentation gather copies it into the
    /// frame's arena.
    local_bones: bone_ease::BoneEase,
    /// Scratch for this frame's resolved bone-offset target, reused so
    /// advancing the local body's easing allocates nothing.
    local_bone_target: Vec<petramond_render::BoneOffset>,
    /// The LOCAL hand's predicted one-shots (the ONLY source of the own
    /// hand animation — the server never echoes self-initiated one-shots)
    /// and the attack follow-through window.
    hand: local_hand::LocalHand,
    /// Evicted replica sections parked for `SectionCached` re-promotion —
    /// harvested by the app shell on disconnect so a reconnect's Join
    /// manifest can claim them.
    pub section_cache: section_cache::SectionCache,
    fallback_world: SurfaceDensitySystem,
    particles: ParticleSystem,
    /// Dust pacing while the local player is actively mining.
    mining_feedback: dig_feedback::DigFeedback,
    /// Dust and dig-hit pacing per digging mob.
    mob_digging: HashMap<u64, dig_feedback::DigFeedback>,
    /// The eased open fraction of every animated block mid-swing (a chest's
    /// lid, a door's or trapdoor's panel), keyed by its anchor cell. Seeded
    /// when a block's logical open state changes, eased by
    /// [`Game::advance_block_animations`], read per frame by the presentation
    /// snapshot through [`Game::block_open_progress`]. Client-side animation
    /// only, never persisted — the authoritative state lives in the cell-state
    /// store and the replicated open-chest set (which it also holds).
    block_animations: block_animation::BlockAnimations,
}

impl Game {
    pub fn set_aspect(&mut self, aspect: f32) {
        self.cam.aspect = aspect;
    }

    /// The player's ear (eye) position, for the app layer's distance
    /// attenuation of positional mod sounds. Movement-derived → the client's
    /// predicted player.
    #[inline]
    pub fn listener_position(&self) -> petramond_math::world_pos::WorldPos {
        self.player.eye()
    }

    /// Current fixed-tick number, exposed for client-side presentation systems
    /// that schedule effects against game tick time without mutating the sim.
    /// The REPLICATED tick (latest `TickUpdate`), not a server-world read.
    #[inline]
    pub fn current_tick(&self) -> u64 {
        self.entities.tick()
    }

    /// The OTHER connected players (id → name). Empty in singleplayer.
    #[cfg(test)]
    pub fn player_roster(&self) -> &HashMap<petramond::player::PlayerId, String> {
        self.entities.roster()
    }

    /// Request a survival/spectator toggle. The in-process listen player is
    /// intrinsically an operator and predicts immediately; TCP clients wait
    /// for the server-authoritative `SelfState::mode`, so an unprivileged
    /// client cannot enter spectator even briefly.
    pub fn toggle_player_mode(&mut self) {
        if !self.net.is_remote() {
            self.player.toggle_mode();
            self.self_view.mode = self.player.mode();
        }
        self.net.queue(ClientToServer::Action(PlayerAction::ToggleMode));
    }

    #[cfg(test)]
    #[inline]
    pub fn player_mode(&self) -> PlayerMode {
        self.player.mode()
    }

    /// The CLIENT-owned active hotbar slot (what the number keys set and the
    /// next `PlayerUpdate` carries).
    #[cfg(test)]
    #[inline]
    pub fn active_hotbar(&self) -> u8 {
        self.player.inventory.active_slot()
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
        self.self_view.apply(state, true);
        if let Some(sync) = sync {
            self.menu_view.apply(sync);
        }
    }

    /// Select hotbar `slot` (number key). Client-owned: the index rides the
    /// next `PlayerUpdate.hotbar_slot`; any hotbar change resets the R-key
    /// rotation cycle (clear-on-select). The replicated view mirrors it
    /// immediately — the server never echoes the index back (a lagged echo
    /// would yank a fast scroll).
    pub fn set_active_hotbar(&mut self, slot: u8) {
        self.player.inventory.set_active(slot);
        self.self_view.inventory.set_active(slot);
        self.held_rotation.clear();
    }

    /// Cycle the held block's placement rotation (the R key). Client-owned:
    /// the armed-item check reads the REPLICATED inventory's selection; the
    /// raw counter rides the next `PlayerUpdate.held_rotation` and the
    /// session re-derives the armed item (see `HeldRotation::apply_wire`).
    pub fn toggle_held_block_rotation(&mut self) {
        let selected = self.self_view.inventory.selected().map(|s| s.item);
        self.held_rotation.toggle(selected);
    }

    /// The in-progress eat progress for the LOCAL player (chew animation),
    /// read from the replicated self view.
    pub fn eating_progress(&self) -> Option<f32> {
        self.self_view.eating
    }

    /// The held block's previewed placement state — the CLIENT's rotation
    /// cycle over the REPLICATED inventory's selected item (the render-path
    /// preview; the session keeps its own latched copy for the actual
    /// placement tick).
    #[inline]
    pub fn held_block_state(&self) -> HeldBlockState {
        self.held_rotation
            .held_block_state(self.self_view.inventory.selected().map(|s| s.item))
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
        self.self_view.sleeping
    }

    /// `(sleeping, total)` across every connected player, the local one
    /// included — the server's headcount, since remote rows only reach this
    /// client for the players in its view. The sleep overlay shows
    /// "x/y players sleeping" from this when `total > 1`.
    pub fn sleeping_player_counts(&self) -> (usize, usize) {
        (
            usize::from(self.entities.sleep_tally().sleeping),
            usize::from(self.entities.sleep_tally().connected),
        )
    }

    /// While sleeping, the engine yaw the lying third-person body's head faces:
    /// from the bed's base (foot) cell toward its pillow cell. `None` while
    /// awake or if the bed vanished mid-sleep. The bed cell is REPLICATED
    /// (`SelfState::sleep_bed`); the bed's model group is read from the
    /// REPLICA (model cells replicate via payload states + deltas).
    pub(super) fn sleep_head_yaw(&self) -> Option<f32> {
        let base = self.self_view.sleep_bed?;
        let (_, _, cells) = self.replica.model_group(base)?;
        let other = cells.iter().copied().find(|c| *c != base)?;
        let d = other - base;
        Some((d.x as f32).atan2(d.z as f32))
    }

    /// The LOCAL player's health for the HUD hearts (replicated self view), or
    /// `None` when there is no survival bar to draw (a floating spectator).
    pub fn player_health(&self) -> Option<petramond_world::gui_state::HealthView> {
        if self.self_view.mode != petramond::player::PlayerMode::Survival {
            return None;
        }
        Some(petramond_world::gui_state::HealthView {
            current: self.self_view.health,
            max: petramond::player::MAX_HEALTH,
        })
    }

    /// The LOCAL player's active status effects for the HUD icon row, in
    /// application order (replicated self view). Empty for a spectator — the
    /// row hides with the hearts.
    pub fn player_effect_icons(&self) -> Vec<petramond_world::effect::Effect> {
        if self.self_view.mode != petramond::player::PlayerMode::Survival {
            return Vec::new();
        }
        self.self_view.effects.iter().map(|&(e, _)| e).collect()
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
        self.look = Some(RaycastHit {
            block,
            normal,
            spot: petramond_math::math::Vec3::splat(0.5),
            outline: petramond_world::selection::SelectionShape::full_block(block),
        });
        // The full click composition (mod interact predictors first), so
        // tests exercise the same walk production clicks run.
        self.predict_use_click(sneak, None).1
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
        self.look = Some(RaycastHit {
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
