//! Voxel game CLIENT session and scene state.
//!
//! `Game` is the client half of the split (multiplayer Phase C2a/C2b): the
//! camera, the locally-predicted player ([`Game::player`] — movement physics
//! and the camera source), per-frame targeting ([`Game::look`]/
//! [`Game::targeted_mob`]), particles, transient animation state, and the
//! app-facing API. The SIMULATION — world, player sessions, entities, the
//! fixed-tick stage ladder — lives in [`crate::server::game::ServerGame`].
//!
//! Since Phase C2b input reaches the sim ONLY as [`crate::net::protocol`]
//! messages: every frame the client translates its input + targeting into a
//! `PlayerUpdate` (+ one-shot `Action`s/`MenuClick`s queued in
//! [`Game::outbox`]) and sends them to the server. Since Phase D the server
//! (`ServerGame`) runs on its OWN self-clocked thread behind a
//! [`ServerHandle`] — the handoff is std::sync::mpsc channels of message
//! VALUES (Arc payloads are refcount bumps); Phase E swaps TCP under the
//! identical messages.
//!
//! The server replies with ordered server→client MESSAGES (terrain payloads +
//! `TickUpdate`s): the client installs terrain into its own REPLICA world
//! ([`Game::replica`] — rendering, collision, raycast, particles, door/chest
//! presentation all read it), entity/self state into the REPLICATED stores
//! ([`Game::self_view`], `replicated.rs`). The client consumes ONLY those
//! messages: the tick's events (world-anchored + self one-shots) and the
//! menu-session view ride the `TickUpdate` (`ClientEvents`/
//! [`Game::menu_view`]); menus open server-side on the tick; tick-side
//! transform mutations come back as `SelfState::transform` corrections;
//! `tick_alpha` is a client-side clock over received updates
//! ([`tick::ReplicaClock`]). MOVEMENT-derived presentation (camera eye,
//! third-person pose) reads `self.player`. The LOCAL player is always
//! session 0 server-side.

pub(crate) mod body_pose;
mod client_presentation;
pub(crate) mod container;
mod environment;
mod frame;
mod local_player;
pub(crate) mod prediction;
pub(crate) mod presentation;
pub(crate) mod remote_players;
pub(crate) mod replicated;
pub(crate) mod session;
mod terrain_render;
mod third_person;
pub(crate) mod tick;

use std::collections::HashMap;

use crate::block_state::HeldBlockState;
use crate::camera::Camera;
use crate::entity::ParticleSystem;
use crate::mathh::IVec3;
use crate::net::protocol::{ChatLine, ClientToServer, MenuSlotWire, PlayerAction, SelfTransform};
#[cfg(test)]
use crate::player::PlayerMode;
use crate::player::{Player, RaycastHit};
use crate::server::handle::ServerHandle;
use crate::server::player::HeldRotation;
use crate::world::World;
use crate::worldgen::density::surface::SurfaceDensitySystem;

pub(crate) use container::ContainerMenu;
pub(crate) use environment::GameEnvironment;
pub(crate) use tick::TickEvents;
pub use tick::{
    GameEvents, GameInput, MobSoundEvent, ModSound, ModSpatialSoundCommand, MovementInput,
    WorldEvent,
};

/// Mining-dust emission interval, seconds.
const MINING_DUST_INTERVAL: f32 = 0.1;

pub struct Game {
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
    /// One-shot client→server messages queued by the app-facing methods since
    /// the last frame, handed to `ServerGame::pump` (after this frame's
    /// `PlayerUpdate` + click edges) in `Game::tick`.
    outbox: Vec<ClientToServer>,
    /// Per-frame scratch for the assembled message batch (capacity reused;
    /// `pump` drains it every frame).
    frame_messages: Vec<ClientToServer>,
    /// Visual-only vertical lag after grounded auto-step movement. The player
    /// feet and collision state update immediately; only the camera eases upward.
    camera_step_y_offset: f32,
    last_player_eye_y: f32,
    /// Third-person view state (boom camera + body pose). `cam` above stays the
    /// authoritative first-person eye for every presentation consumer; see
    /// `third_person.rs`.
    third_person: third_person::ThirdPerson,
    /// The handle to the SIMULATION — `ServerGame` on its own self-clocked
    /// thread (multiplayer Phase D). Input reaches it only as messages
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
    /// proper screen in Phase E.
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
    /// When the currently-open streaming batch's `StreamBatchStart` was
    /// applied; `StreamBatchEnd` closes it into a rate sample and an ack.
    stream_batch_started: Option<std::time::Instant>,
    /// EMA over measured batch apply rates (streaming messages/second) — what
    /// `StreamBatchAck` reports so the server sizes future batches to this
    /// client's real throughput.
    stream_rate_ema: Option<f32>,
    /// Per-frame scratch for drained server messages (capacity reused).
    incoming: Vec<crate::net::protocol::ServerToClient>,
    /// Replica sections installed during the current message drain. Their
    /// overlapping mesh invalidations are applied once after the batch.
    remote_section_installs: Vec<crate::chunk::SectionPos>,
    /// Chat lines received from the server and not yet adopted by the app's
    /// client-side chat history.
    pending_chat_lines: Vec<ChatLine>,
    /// The client's REPLICA world (role `ClientReplica`): installed from the
    /// server's terrain payloads + deltas, it owns light + meshes for the
    /// renderer and answers every client-side world read — collision, raycast,
    /// particles, door/chest presentation, environment sampling.
    replica: World,
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
    /// The client's replicated MENU-session view (`MenuSyncMsg`, on-change):
    /// the EXCLUSIVE source `menu_read_model` renders container screens from.
    menu_view: replicated::MenuView,
    /// This frame's replicated tick events (world-anchored + self one-shots +
    /// sound queues), buffered by `apply_tick_update` and drained once per
    /// `Game::tick` into `GameEvents`.
    pending_events: tick::ClientEvents,
    /// The LOCAL player's server-assigned id (`JoinData::player_id`;
    /// in-process always session 0's) — distinguishes own vs foreign
    /// `ItemPickedUp` events.
    self_id: crate::server::player::PlayerId,
    /// The OTHER connected players (id → name): seeded from
    /// `JoinData::players` on a remote join, then maintained by
    /// `PlayerJoined`/`PlayerLeft` broadcasts on every connection kind.
    player_roster: HashMap<crate::server::player::PlayerId, String>,
    /// REPLICATED remote-player store (Phase F): every OTHER session's
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
    pub(crate) prediction: prediction::PredictionLedger,
    /// Local mining timer for crack overlay + `BreakFinished` (P2).
    local_mining: crate::mining::MiningState,
    /// The movement `Input` this frame's local physics consumed
    /// (`tick_player`) — reused verbatim by `build_player_update` so the wire
    /// intent can never drift from what the prediction simulated.
    predicted_input: crate::player::Input,
    /// One-shot hand/presentation triggers latched this frame for P0
    /// prediction — the ONLY source of the own hand animation (the server
    /// never echoes self-initiated one-shots back; see
    /// WIKI/client-prediction.md). Consumed into `GameEvents` in
    /// `tick_receive`.
    local_hand_jab: bool,
    local_hand_swing: bool,
    local_hand_threw: bool,
    /// The block the LOCAL mining timer finished this frame (hand pop).
    local_broke_block: Option<crate::block::Block>,
    /// The block the place ghost predicted this frame (hand pop).
    local_placed_block: Option<crate::block::Block>,
    /// Optimistic place cell (cleared on accept/deny or replica delta).
    place_ghost: Option<(IVec3, u8)>,
    /// Cells this client already presented place/break for (local WorldEvent).
    /// Wire `BlockPlaced` / `BlockBroken` for these cells are dropped until the
    /// matching outcome clears the entry — never re-play sound/particles for
    /// an optimistic action. Observers' breaks never enter this set.
    predicted_presentation_cells: rustc_hash::FxHashSet<IVec3>,
    fallback_world: SurfaceDensitySystem,
    particles: ParticleSystem,
    /// Wall-clock seconds banked toward the next mining-dust fleck while the
    /// local player is actively mining (client presentation pacing).
    mining_dust_t: f32,
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
    /// authoritative open/closed bit lives in the chunk door map. See [`crate::door`].
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
    pub fn listener_position(&self) -> crate::mathh::Vec3 {
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
    #[allow(dead_code)] // first consumers: Phase E2 (player list) / Phase F (rendering)
    pub(crate) fn player_roster(&self) -> &HashMap<crate::server::player::PlayerId, String> {
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
    pub(crate) fn player_mode(&self) -> PlayerMode {
        self.player.mode()
    }

    /// The CLIENT-owned active hotbar slot (what the number keys set and the
    /// next `PlayerUpdate` carries).
    #[cfg(test)]
    #[inline]
    pub(crate) fn active_hotbar(&self) -> u8 {
        self.player.inventory.active_slot()
    }

    /// Take the queued one-shot messages, for a test harness that services
    /// the server end synchronously (`Game::tick` sends them in play).
    #[cfg(test)]
    pub(crate) fn take_outbox_for_test(&mut self) -> Vec<ClientToServer> {
        std::mem::take(&mut self.outbox)
    }

    /// Apply replicated view refreshes a test harness built server-side,
    /// standing in for the next batch (`SelfState` + optional menu sync).
    #[cfg(test)]
    pub(crate) fn apply_views_for_test(
        &mut self,
        state: &crate::net::protocol::SelfState,
        sync: Option<crate::net::protocol::MenuSyncMsg>,
    ) {
        self.self_view.apply(state);
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
    pub(crate) fn held_block_state(&self) -> HeldBlockState {
        self.held_rotation
            .held_block_state(self.self_view.inventory.selected().map(|s| s.item))
    }

    // --- App-facing action methods. The pub surface `Game` exposed before the
    // client/server split stays intact, but since Phase C2b these PUSH
    // MESSAGES into `outbox` (consumed by `ServerGame::pump` inside
    // `Game::tick`) instead of touching server state. Since C2c-iii the menu
    // read model renders from the REPLICATED `MenuView` and the screen-open
    // calls are requests/acks (menus open server-side on the tick).

    /// Ask the server to persist everything (world chunks, level.dat,
    /// per-player files, mod set) — a control message; the save happens on the
    /// server thread. For the QUIT path use [`Game::shutdown`], which also
    /// joins the thread.
    pub fn save_all(&mut self) {
        self.handle.save_all();
    }

    /// Quit this session: the server thread saves everything (what `save_all`
    /// did) and exits; returns once it is joined.
    pub fn shutdown(mut self) {
        self.handle.shutdown_and_join();
    }

    /// Singleplayer pause (the pause menu): the server keeps draining
    /// messages, streaming, and autosaving, but skips the fixed ticks and
    /// banks no tick debt. Honored server-side only while it has never been
    /// open to LAN (the sole connection is this local one); once opened, the
    /// server ignores Pause. While paused the app must keep calling
    /// [`Game::pump_network`] so server output is still consumed.
    pub fn set_paused(&mut self, paused: bool) {
        // A remote client never pauses the shared server (which also gates:
        // once opened to LAN, Pause is ignored) — belt and braces.
        if self.remote {
            return;
        }
        if self.handle.send(ClientToServer::Pause(paused)).is_err() {
            self.note_connection_lost();
        }
    }

    pub fn send_chat(&mut self, text: String) {
        self.outbox.push(ClientToServer::ChatSend { text });
    }

    pub(crate) fn take_chat_lines(&mut self) -> Vec<ChatLine> {
        std::mem::take(&mut self.pending_chat_lines)
    }

    /// Whether this session fronts a REMOTE server (joined over TCP) rather
    /// than the in-process host thread.
    #[inline]
    pub fn is_remote(&self) -> bool {
        self.remote
    }

    /// Open the running HOST server to LAN on `port`; `Ok` carries the
    /// actual bound port. Host only — the pause menu hides the button for
    /// remote sessions (and a remote handle has no control channel to ask).
    pub fn open_to_lan(&mut self, port: u16) -> std::io::Result<u16> {
        debug_assert!(!self.remote, "open_to_lan is a host action");
        self.handle.open_to_lan(port)
    }

    /// One-shot: the latched connection-loss reason if it has not yet been
    /// surfaced. `Game::tick` reports through `GameEvents::connection_lost`;
    /// the app polls THIS on frames that skip the tick (shell screens over a
    /// live game — the pause menu) so a loss detected by
    /// [`Game::pump_network`] still reaches the Disconnected screen.
    pub fn take_connection_lost(&mut self) -> Option<String> {
        if self.connection_lost_reported {
            return None;
        }
        let reason = self.connection_lost.clone()?;
        self.connection_lost_reported = true;
        log::error!("{reason}; nothing further will be saved");
        Some(reason)
    }

    /// Latch the server as unreachable (crashed thread / closed channel /
    /// lost TCP connection); reported exactly once through
    /// `GameEvents::connection_lost`.
    fn note_connection_lost(&mut self) {
        self.note_connection_lost_because("world stopped: the server is gone");
    }

    /// [`note_connection_lost`](Self::note_connection_lost) with an explicit
    /// reason (`ServerClosing` / a server `Disconnect`); the first reason
    /// latched wins.
    fn note_connection_lost_because(&mut self, reason: &str) {
        if self.connection_lost.is_none() {
            self.connection_lost = Some(reason.to_string());
        }
    }

    /// Snapshot the predicted inventory and open a ledger entry for one
    /// predicted mutating action: `(can, id)`. When `can` is false the entry
    /// is track-only (no snapshot) and the caller must skip its local
    /// mutation — the ledger is at capacity until the server catches up.
    fn begin_inventory_prediction(&mut self) -> (bool, crate::net::protocol::ClientRequestId) {
        let can = self.prediction.can_predict();
        let snapshot = if can {
            prediction::PredictionSnapshot::Inventory(self.self_view.inventory.clone())
        } else {
            prediction::PredictionSnapshot::None
        };
        (can, self.prediction.begin(snapshot))
    }

    /// Drop the player's held (active hotbar) item into the world via the in-game
    /// drop key. With `all`, the whole stack is thrown (Ctrl+Q); otherwise a
    /// single item (Q). No-op with an empty hand.
    pub fn drop_selected_item(&mut self, all: bool) {
        // P0 throw animation is client-owned: trigger when the hand holds
        // anything (the server never echoes the one-shot back).
        let slot = self.self_view.inventory.active_slot() as usize;
        self.local_hand_threw |= self.self_view.inventory.slot(slot).is_some();
        let (can, request_id) = self.begin_inventory_prediction();
        if can {
            let slot = self.self_view.inventory.active_slot() as usize;
            if all {
                let _ = self
                    .self_view
                    .inventory
                    .slot_mut(slot)
                    .and_then(|c| c.take());
            } else if let Some(cell) = self.self_view.inventory.slot_mut(slot) {
                if let Some(stack) = cell.as_mut() {
                    stack.count = stack.count.saturating_sub(1);
                    if stack.count == 0 {
                        *cell = None;
                    }
                }
            }
        }
        self.outbox.push(ClientToServer::Action(PlayerAction::Drop {
            all,
            request_id,
        }));
    }

    /// Throw the whole cursor-held stack out into the world (inventory drag-out
    /// then click outside the panel). No-op when the cursor is empty.
    pub fn throw_cursor_stack(&mut self) {
        self.local_hand_threw |= self.self_view.inventory.cursor().is_some();
        let (can, request_id) = self.begin_inventory_prediction();
        if can {
            *self.self_view.inventory.cursor_mut() = None;
        }
        self.outbox
            .push(ClientToServer::Action(PlayerAction::ThrowCursorStack {
                request_id,
            }));
    }

    /// Throw a single item off the cursor-held stack (right-click outside the
    /// panel while dragging). No-op when the cursor is empty.
    pub fn throw_cursor_one(&mut self) {
        self.local_hand_threw |= self.self_view.inventory.cursor().is_some();
        let (can, request_id) = self.begin_inventory_prediction();
        if can {
            if let Some(cur) = self.self_view.inventory.cursor_mut().as_mut() {
                cur.count = cur.count.saturating_sub(1);
                if cur.count == 0 {
                    *self.self_view.inventory.cursor_mut() = None;
                }
            }
        }
        self.outbox
            .push(ClientToServer::Action(PlayerAction::ThrowCursorOne {
                request_id,
            }));
    }

    /// Latch a hit-tested container click for the next game tick: resolved by
    /// the App to a [`MenuSlot`](crate::gui::MenuSlot), a button, Shift, and
    /// its double-click `gather` verdict, shipped as a `MenuClick` message and
    /// applied in arrival order by the tick's menu stage. Optimistically
    /// mutates the predicted inventory when the ledger has room.
    pub fn menu_click(
        &mut self,
        slot: crate::gui::MenuSlot,
        button: crate::controls::PointerButton,
        shift: bool,
        gather: bool,
    ) {
        // Clicks the prediction cannot faithfully apply ride track-only: no
        // inventory clone, no snapshot slot burned, nothing to roll back.
        let (can, request_id) = if self.menu_click_is_predictable(slot, shift, gather) {
            self.begin_inventory_prediction()
        } else {
            (false, self.prediction.begin_track_only())
        };
        if can {
            self.predict_menu_click(slot, button, shift, gather);
        }
        self.outbox.push(ClientToServer::MenuClick {
            slot: MenuSlotWire::from_menu_slot(&slot),
            button: crate::net::protocol::button_to_wire(button),
            shift,
            gather,
            request_id,
        });
    }

    /// Whether a click's outcome is container-independent — the ONLY clicks
    /// the client may predict without a local container mirror, because the
    /// shared apply (`ContainerMenu::click`) reroutes the rest by the open
    /// target: shift-clicks route into an open chest/furnace/mod/workbench,
    /// and a gather sweeps an open block container. Predicting those with
    /// inventory-only primitives would drift from the server (the
    /// single-apply-path rule in WIKI/client-prediction.md).
    fn menu_click_is_predictable(
        &self,
        slot: crate::gui::MenuSlot,
        shift: bool,
        gather: bool,
    ) -> bool {
        if !matches!(slot, crate::gui::MenuSlot::Inventory(_)) {
            return false;
        }
        let v = &self.menu_view;
        let block_container_open =
            v.chest.is_some() || v.furnace.is_some() || v.container.is_some();
        if shift {
            !block_container_open && v.workbench.is_none()
        } else if gather {
            !block_container_open
        } else {
            true
        }
    }

    /// Apply inventory-only click prediction; callers gate on
    /// [`menu_click_is_predictable`](Self::menu_click_is_predictable), so
    /// every arm here matches what `ContainerMenu::click` will do server-side.
    fn predict_menu_click(
        &mut self,
        slot: crate::gui::MenuSlot,
        button: crate::controls::PointerButton,
        shift: bool,
        gather: bool,
    ) {
        use crate::controls::PointerButton;
        use crate::gui::MenuSlot;
        let inv = &mut self.self_view.inventory;
        match slot {
            MenuSlot::Inventory(i) => {
                if shift {
                    inv.shift_move_slot(i);
                } else if gather {
                    inv.collect_to_cursor();
                } else {
                    match button {
                        PointerButton::Primary => inv.click_slot(i),
                        PointerButton::Secondary => inv.right_click_slot(i),
                    }
                }
            }
            _ => {}
        }
    }

    /// Whether the LOCAL cursor currently holds a stack, from the REPLICATED
    /// inventory (cursor rides `SelfState`). Gates the double-click gather,
    /// which only fires while a stack is being dragged; the gather verdict
    /// ships in the `MenuClick` message.
    pub fn cursor_has_stack(&self) -> bool {
        self.self_view.inventory.cursor().is_some()
    }

    /// Read-only state needed to build the UI snapshot for the LOCAL player's
    /// current menu — assembled ENTIRELY from the replicated stores: the
    /// inventory from `SelfView`, the menu-session views (craft grid, furnace,
    /// chest, workbench, mod GUI slots + state map) from the `MenuView` the
    /// per-tick `MenuSyncMsg`s feed. No server-session reads.
    pub fn menu_read_model(&self) -> crate::server::menu::MenuReadModel<'_> {
        let view = &self.menu_view;
        crate::server::menu::MenuReadModel {
            inventory: &self.self_view.inventory,
            craft: &view.craft,
            furnace: view.furnace,
            chest: view.chest,
            workbench: view.workbench.clone(),
            gui_state: view.gui_state.clone(),
            container: view.container.clone(),
        }
    }

    // Menu sessions open SERVER-SIDE on the tick (at the interaction / mod
    // action / OpenInventory request sites) since C2c-iii. The App's open_*
    // calls below keep their historical signatures: `open_crafting(2)` (the E
    // key) is the one genuine REQUEST — it sends `PlayerAction::OpenInventory`
    // — while the rest are client-side ACKNOWLEDGMENTS the App makes when it
    // receives the matching `GameEvents.open_*` one-shot (by which point the
    // server session is already open). Since Phase D the client holds no
    // session state to assert against, so the acks are plain no-ops.

    /// The E-key inventory screen (`cols == 2`): REQUEST the server open the
    /// 2×2 crafting session on the next tick. `cols == 3` is the App's ack of
    /// a server-opened crafting-table session (no-op; see above).
    pub fn open_crafting(&mut self, cols: usize) {
        if cols <= 2 {
            self.outbox
                .push(ClientToServer::Action(PlayerAction::OpenInventory));
        }
    }

    /// Ack of a server-opened furnace session at `pos` (no-op; see above).
    pub fn open_furnace_screen(&mut self, pos: IVec3) {
        let _ = pos;
    }

    /// Ack of a server-opened chest session at `pos` (no-op; see above).
    pub fn open_chest_screen(&mut self, pos: IVec3) {
        let _ = pos;
    }

    /// Ack of a server-opened furniture-workbench session (no-op; see above).
    pub fn open_workbench_screen(&mut self) {}

    /// Ack of a server-opened mod GUI session (no-op; see above).
    pub fn open_mod_gui_screen(&mut self, kind: crate::gui::GuiKind, pos: Option<IVec3>) {
        let _ = (kind, pos);
    }

    /// Close the LOCAL player's open menu session. The server-side close
    /// (cursor stash, craft-grid return, viewer release) runs ON THE TICK the
    /// message lands on; there is no client-side menu state to clear — the App
    /// owns which screen is up.
    pub fn close_open_menu(&mut self) {
        self.outbox
            .push(ClientToServer::Action(PlayerAction::CloseMenu));
    }

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
    pub fn player_health(&self) -> Option<crate::gui::HealthView> {
        if self.self_view.mode == crate::player::PlayerMode::Spectator {
            return None;
        }
        Some(crate::gui::HealthView {
            current: self.self_view.health,
            max: crate::player::MAX_HEALTH,
        })
    }

    /// The LOCAL player's active status effects for the HUD icon row, in
    /// application order (replicated self view). Empty for a spectator — the
    /// row hides with the hearts.
    pub fn player_effect_icons(&self) -> Vec<crate::effect::Effect> {
        if self.self_view.mode == crate::player::PlayerMode::Spectator {
            return Vec::new();
        }
        self.self_view.effects.iter().map(|&(e, _)| e).collect()
    }

    /// Test injection: set the client's look target without a raycast, then
    /// run place prediction (the production path after `refresh_target`).
    #[cfg(test)]
    pub(crate) fn predict_place_at_for_test(
        &mut self,
        block: IVec3,
        normal: IVec3,
        sneak: bool,
    ) -> Option<crate::net::protocol::ClientRequestId> {
        self.look = Some(RaycastHit {
            block,
            normal,
            outline: crate::mathh::SelectionShape::full_block(block),
        });
        self.try_predict_place_ghost(sneak)
    }

    /// Test injection: run the full local break prediction at `pos`.
    #[cfg(test)]
    pub(crate) fn predict_break_at_for_test(&mut self, pos: IVec3, block: crate::block::Block) {
        self.apply_predicted_break(pos, block, Some(IVec3::Y));
    }
}

#[cfg(test)]
mod tests;
