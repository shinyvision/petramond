//! Voxel game CLIENT session and scene state.
//!
//! `Game` is the client half of the client/server split: the
//! camera, the locally-predicted player (`Game::player` — movement physics
//! and the camera source), per-frame targeting (`Game::look`/
//! `Game::targeted_mob`), particles, transient animation state, and the
//! app-facing API. The SIMULATION — world, player sessions, entities, the
//! fixed-tick stage ladder — lives in [`petramond::server::game::ServerGame`].
//!
//! Input reaches the sim ONLY as [`petramond::net::protocol`]
//! messages: every frame the client translates its input + targeting into a
//! `PlayerUpdate` (+ one-shot `Action`s/menu actions queued in
//! `Game::outbox`) and sends them to the server. The server
//! (`ServerGame`) runs on its OWN self-clocked thread behind a
//! [`ServerHandle`] — the handoff is std::sync::mpsc channels of message
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
mod local_player;
mod menu_actions;
mod menu_prediction;
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
pub mod tick;
mod view_bob;
mod world_prediction;
pub mod world_tool;

use std::collections::{HashMap, VecDeque};

use crate::particle::ParticleSystem;
use petramond::net::protocol::{ChatLine, ClientToServer, PlayerAction, SelfTransform};
#[cfg(test)]
use petramond::player::PlayerMode;
use petramond::player::{Player, RaycastHit};
use petramond::server::handle::ServerHandle;
use petramond::server::player::HeldRotation;
use petramond::world::World;
use petramond_math::math::IVec3;
use petramond_render::camera::Camera;
use petramond_world::block_state::HeldBlockState;
use petramond_worldgen::density::surface::SurfaceDensitySystem;

pub use environment::GameEnvironment;
pub use frame::{render_bone_offsets, render_held_pose};
pub use tick::{
    GameEvents, GameInput, MobSoundEvent, MovementInput, SpatialSoundCommand, WorldEvent,
};

pub struct Game {
    /// The shared background pool the replica streams on; presentation jobs
    /// (ghost meshes, library I/O) run there too.
    jobs: std::sync::Arc<petramond::worker::JobPool>,
    /// A refusal or failure to show the player once; the app takes it.
    pub notice: String,
    pub world_tools: world_tool::WorldTools,
    pub schematic_preview: schematic_preview::SchematicPreview,
    pub schematic_library: schematic_library::SchematicLibrary,
    /// A paste the player asked for went up (an edge the menu closes on).
    paste_preview_ready: bool,
    /// Captures arriving from, and pastes leaving for, the server.
    flight_toggle: creative::FlightToggle,
    break_repeat: creative::BreakRepeat,
    pub schematics: schematics::SchematicShare,
    ghosts: ghosts::Ghosts,
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
    /// The authoritative mount from the own replicated player row —
    /// `(stable mob id, seat index)`. While `Some`, local player physics and
    /// entity push are suspended and the body slaves per frame to the
    /// interpolated mount at the species' seat offset; movement INTENT keeps
    /// riding `PlayerUpdate` so the driving mod reads it server-side.
    self_mount: Option<petramond::net::protocol::PlayerMount>,
    /// The client-owned R-key placement-rotation cycle; its raw counter rides
    /// `PlayerUpdate.held_rotation` (the session keeps its own latched copy).
    held_rotation: HeldRotation,
    /// One-shot client→server messages queued by the app-facing methods since
    /// the last frame, handed to `ServerGame::pump` (after this frame's
    /// `PlayerUpdate` + click edges) in `Game::tick`.
    outbox: Vec<ClientToServer>,
    /// Per-frame scratch for the assembled message batch (capacity reused;
    /// `pump` drains it every frame).
    frame_messages: Vec<ClientToServer>,
    /// Visual-only vertical lag after grounded auto-step movement. The player
    /// feet and collision state update immediately; only the camera eases —
    /// upward after a step-up, downward after a sneak snap-down.
    camera_step_y_offset: f32,
    /// Visual-only eased eye drop while sneaking (`0` upright …
    /// `-SNEAK_EYE_DROP` crouched) — the first-person feedback that sneak is
    /// active. Camera only: the collision box and the sim eye stay full
    /// height.
    camera_sneak_y_offset: f32,
    last_player_eye_y: f64,
    /// Third-person view state (boom camera + body pose). `cam` above stays the
    /// authoritative first-person eye for every presentation consumer; see
    /// `third_person.rs`.
    third_person: third_person::ThirdPerson,
    /// The handle to the SIMULATION — `ServerGame` on its own self-clocked
    /// thread. Input reaches it only as messages
    /// ([`ServerHandle::send`]); state comes back only as drained
    /// server→client messages. The client holds NO direct sim state.
    handle: ServerHandle,
    /// Whether this session is a REMOTE client (built by
    /// [`Game::new_remote`] over a TCP connection). Gates host-only actions:
    /// pause, open-to-LAN, save-and-quit.
    remote: bool,
    /// `Some(reason)` once the server is unreachable (thread crashed, or a
    /// send/drain hit a closed channel). Latched once, surfaced through
    /// `GameEvents::connection_lost` on the frame it is detected; the app
    /// keeps running the (frozen) world until it consumes the event with a
    /// proper connection-lost screen.
    connection_lost: Option<String>,
    /// Whether `connection_lost` was already surfaced (log + event) — the
    /// error is reported exactly once.
    connection_lost_reported: bool,
    /// The transform of the last `PlayerUpdate` this client SENT. A
    /// `SelfState::transform` correction adopts only the fields that differ
    /// from it: fields equal to what we last claimed are just the server
    /// echoing us, and the local (possibly newer) value wins.
    last_sent_transform: Option<SelfTransform>,
    /// Client-side tick clock over RECEIVED `TickUpdate`s — the `tick_alpha`
    /// source now that the server accumulator lives on another thread.
    replica_clock: tick::ReplicaClock,
    /// Bounded FIFO of `TickUpdate` entity rows waiting for crossed render-time
    /// segment boundaries (see `ReplicaClock` / `StagedRows`).
    staged_rows: VecDeque<replicated::StagedRows>,
    /// When the currently-open streaming batch's `StreamBatchStart` was
    /// applied; `StreamBatchEnd` closes it into a rate sample and an ack.
    stream_batch_started: Option<std::time::Instant>,
    /// EMA over measured batch apply rates (streaming messages/second) — what
    /// `StreamBatchAck` reports so the server sizes future batches to this
    /// client's real throughput.
    stream_rate_ema: Option<f32>,
    stream_feedback_at: Option<std::time::Instant>,
    /// Per-frame scratch for drained server messages (capacity reused).
    incoming: Vec<petramond::net::protocol::ServerToClient>,
    /// Replica sections installed during the current message drain. Their
    /// overlapping mesh invalidations are applied once after the batch.
    remote_section_installs: Vec<petramond_world::chunk::SectionPos>,
    /// Chat lines received from the server and not yet adopted by the app's
    /// client-side chat history.
    pending_chat_lines: Vec<ChatLine>,
    /// The client's REPLICA world (role `ClientReplica`): installed from the
    /// server's terrain payloads + deltas, it owns light + meshes for the
    /// renderer and answers every client-side world read — collision, raycast,
    /// particles, door/chest presentation, environment sampling.
    replica: World,
    /// Optional presentation-only client WASM modules. They read the replica
    /// and publish document state/images; they never share the server mod
    /// instances or simulation mutation seams.
    client_mods: petramond::modding::client::ClientModRuntime,
    /// REPLICATED mob store: presentation reads these, fed by the per-tick
    /// `TickUpdate` batches — never `server.world.mobs()` (see
    /// `game/replicated.rs`).
    replicated_mobs: replicated::ReplicatedMobs,
    /// REPLICATED dropped-item store (same contract as `replicated_mobs`).
    replicated_items: replicated::ReplicatedItems,
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
    /// The LOCAL player's server-assigned id (`JoinData::player_id`;
    /// in-process always session 0's) — distinguishes own vs foreign
    /// `ItemPickedUp` events.
    self_id: petramond::player::PlayerId,
    /// The OTHER connected players (id → name): seeded from
    /// `JoinData::players` on a remote join, then maintained by
    /// `PlayerJoined`/`PlayerLeft` broadcasts on every connection kind.
    player_roster: HashMap<petramond::player::PlayerId, String>,
    /// REPLICATED remote-player store: every OTHER session's
    /// prev/curr row pair plus its body-pose / held-item animation state —
    /// what `collect_remote_players` renders bodies from. The local player
    /// is never in it.
    remote_players: remote_players::RemotePlayers,
    /// The latest replicated tick number (`TickUpdate::tick`) — the client's
    /// notion of game time for presentation scheduling.
    replicated_tick: u64,
    /// Chests with at least one open screen anywhere (replicated per batch —
    /// the server's `chest_viewers` key set). Drives the lid animation.
    open_chests: rustc_hash::FxHashSet<IVec3>,
    /// Optimistic prediction ledger (request ids + undo snapshots).
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
    /// First-person walking sway — a presentation offset on the camera, and
    /// the stride phase the first-person animator's walk plays on.
    view_bob: view_bob::ViewBob,
    /// The look's turn rates, advanced once per frame with the look.
    first_person_look: first_person::LookRate,
    /// Speed-coupled FOV — the camera widens with the body's WISHED land
    /// speed (`Player::wish_speed`), a presentation retarget of `cam.fov_y`.
    speed_fov: speed_fov::SpeedFov,
    /// One-shot hand/presentation triggers latched this frame for P0
    /// prediction — the ONLY source of the own hand animation (the server
    /// never echoes self-initiated one-shots back). Consumed into `GameEvents` in
    /// `tick_receive`.
    local_hand_jab: bool,
    /// The latched jab belongs to the LEFT hand (the use-click prediction's
    /// off-hand pass produced it). Meaningless while `local_hand_jab` is
    /// false.
    local_hand_jab_off: bool,
    /// The latched consumed click's consumer presents itself (an eat's
    /// raise), so the hand plays no jab. Meaningless while `local_hand_jab`
    /// is false.
    local_hand_presents_itself: bool,
    /// The latched consumed click is a placement: the place jab plays, not
    /// the interact one. Meaningless while `local_hand_jab` is false.
    local_hand_places: bool,
    local_hand_swing: bool,
    /// Seconds of the local hand's swing still to FOLLOW THROUGH: the
    /// client's mirror of the server's attack cooldown, armed by the same
    /// attribute-scaled window. Without it the press predicted a swing the
    /// server's cooldown was about to refuse, so a mash restarted the
    /// animation mid-arc while the hits kept the server's pace.
    local_attack_recovery: f32,
    /// A press held over the follow-through, ONE deep: it fires by itself
    /// the frame the recovery ends (see `attack_press`), so a mash chains
    /// without having to land on the beat.
    local_attack_queued: bool,
    local_hand_threw: bool,
    /// Hand-swing one-shots latched at event assembly for the client-mod
    /// frame hook (the ABI's swing facts, `PlayerSnapshot::swing`) and taken
    /// by `drive_client_mods`. Its own latch, deliberately: the app's hand
    /// events feed the animators and drain at RENDER — a different
    /// clock — and a shared latch is whoever-eats-first, which once left a
    /// swing-claim pack dark on every one-shot. `mining` is unused here (the
    /// level is read live at dispatch, like the server's roster build).
    swing_events: mod_api::HandSwing,
    /// The block the LOCAL mining timer finished this frame (hand pop).
    local_broke_block: Option<petramond_world::block::Block>,
    /// The block the place ghost predicted this frame (hand pop).
    local_placed_block: Option<petramond_world::block::Block>,
    /// The predicted place committed from the OFF hand (left-hand pop).
    local_placed_off_hand: bool,
    /// Optimistic place cell (cleared on accept/deny or replica delta).
    place_ghost: Option<(IVec3, u16)>,
    /// Cells this client already presented place/break for (local WorldEvent).
    /// Wire `BlockPlaced` / `BlockBroken` for these cells are dropped until the
    /// matching outcome clears the entry — never re-play sound/particles for
    /// an optimistic action. Observers' breaks never enter this set.
    predicted_presentation_cells: rustc_hash::FxHashSet<IVec3>,
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
    /// Transient per-chest lid open angle (`0.0` closed .. `1.0` open), keyed by world
    /// position. Eased toward open for the chest whose screen is up and toward closed
    /// for the rest; client-side animation only, never persisted. The render-side
    /// presentation snapshot reads the angle (via [`Game::chest_lid_angle`]) to bake the lid;
    /// the easing in [`Game::advance_chest_lids`] is the owning sim/animation state.
    chest_lids: HashMap<IVec3, f32>,
    /// Transient per-door swing angle (`0.0` closed .. `1.0` open), keyed by the door's
    /// LOWER cell. A door enters the map when right-click toggles it and is eased toward
    /// its (now flipped) logical open state by [`Game::advance_door_swings`]; once it
    /// reaches the target it is dropped (the renderer then reads the resting angle
    /// straight from the door state). Client-side animation only, never persisted — the
    /// authoritative open/closed bit lives in the chunk door map. See [`petramond_world::door`].
    door_swings: HashMap<IVec3, f32>,
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
        self.replicated_tick
    }

    /// The OTHER connected players (id → name). Empty in singleplayer.
    #[cfg(test)]
    pub fn player_roster(&self) -> &HashMap<petramond::player::PlayerId, String> {
        &self.player_roster
    }

    /// Request a survival/spectator toggle. The in-process listen player is
    /// intrinsically an operator and predicts immediately; TCP clients wait
    /// for the server-authoritative `SelfState::mode`, so an unprivileged
    /// client cannot enter spectator even briefly.
    pub fn toggle_player_mode(&mut self) {
        if !self.remote {
            self.player.toggle_mode();
            self.self_view.mode = self.player.mode();
        }
        self.outbox
            .push(ClientToServer::Action(PlayerAction::ToggleMode));
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
        std::mem::take(&mut self.outbox)
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
    // client/server split stays intact, but these PUSH
    // MESSAGES into `outbox` (consumed by `ServerGame::pump` inside
    // `Game::tick`) instead of touching server state. The menu
    // read model renders from the REPLICATED `MenuView` and the screen-open
    // calls are requests/acks (menus open server-side on the tick).

    /// App-side wake request (ESC / "Leave bed"), latched to the next tick.
    pub fn request_wake(&mut self) {
        self.outbox.push(ClientToServer::Action(PlayerAction::Wake));
    }

    /// App-side respawn request (the death screen's button), latched to the
    /// next tick.
    pub fn request_respawn(&mut self) {
        self.outbox
            .push(ClientToServer::Action(PlayerAction::Respawn));
    }

    /// Sleep fade progress in `[0, 1]` while the LOCAL player sleeps — the read
    /// model the presentation overlay darkens by, from the replicated self
    /// view. `None` while awake.
    pub fn sleep_progress01(&self) -> Option<f32> {
        self.self_view.sleeping
    }

    /// `(sleeping, total)` across every connected player — the replicated
    /// remote rows plus the local self view. The sleep overlay shows
    /// "x/y players sleeping" from this when `total > 1`.
    pub fn sleeping_player_counts(&self) -> (usize, usize) {
        let self_sleeping = usize::from(self.self_view.sleeping.is_some());
        (
            self.remote_players.sleeping_count() + self_sleeping,
            self.remote_players.len() + 1,
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
            outline: petramond_math::math::SelectionShape::full_block(block),
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
            outline: petramond_math::math::SelectionShape::full_block(block),
        });
        let mut input = crate::game::tick::GameInput::default();
        input.movement.sneak = sneak;
        self.predict_click_verdict(&input, None)
    }
}

#[cfg(test)]
mod tests;
