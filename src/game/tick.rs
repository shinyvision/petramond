//! The frame→tick boundary types shared by the client and the server sim
//! ([`GameInput`], [`GameEvents`], [`TickEvents`]/[`WorldEvents`]) plus the
//! client's per-frame [`Game::tick`] driver. The fixed-tick stage ladder
//! itself lives on [`crate::server::game::ServerGame`].

use super::Game;
use crate::block::Block;
use crate::mathh::{IVec3, Vec3};
use crate::net::protocol::{
    ClientToServer, OpenScreen, PlayerAction, PlayerUpdate, SelfEvents, TargetRef,
};
use crate::server::player::PlayerId;

use super::prediction;

/// Fixed simulation timestep: 20 game ticks per second, independent of frame
/// rate. World simulation (block updates, scheduled ticks, water flow) advances
/// in whole steps of this size.
pub(crate) const TICK_DT: f32 = 0.05;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct MovementInput {
    pub forward: bool,
    pub backward: bool,
    pub left: bool,
    pub right: bool,
    pub jump: bool,
    pub sneak: bool,
    pub sprint: bool,
}

#[derive(Copy, Clone, Debug, Default)]
pub struct GameInput {
    /// False while an app screen such as inventory owns input focus.
    pub gameplay_enabled: bool,
    pub movement: MovementInput,
    pub look_delta: (f32, f32),
    /// Whole wheel notches scrolled this frame (signed): negative selects
    /// previous slots, positive selects next, 0 for none. Wraps within the hotbar.
    pub hotbar_scroll: i32,
    /// Level state: primary button held for mining.
    pub break_held: bool,
    /// Edge state: primary button *pressed* this frame.
    pub attack_clicked: bool,
    /// Edge state: secondary button pressed for placement.
    pub place_clicked: bool,
    /// Level state: secondary button held — sustains an in-progress eat.
    pub use_held: bool,
}

/// One sound a mod emitted on the tick (`EmitSound` HostCall): resolved to a
/// runtime [`Sound`](crate::audio::Sound) id at call time, carried through the
/// tick→presentation channel, and played by the app layer each frame — the sim
/// never touches audio. `pos` is where it happened (`None` = non-spatial);
/// positional reach comes from the sound row's `attenuation_distance`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ModSound {
    pub sound: crate::audio::Sound,
    pub pos: Option<crate::mathh::Vec3>,
}

/// A semantic mob sound event produced by gameplay. The app resolves the
/// species' `mobs.json` sound hook and owns actual playback.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MobSoundEvent {
    pub mob_id: u64,
    pub kind: crate::mob::Mob,
    pub category: crate::mob::MobSoundCategory,
    pub pos: crate::mathh::Vec3,
}

/// A deterministic presentation command produced by the spatial sound HostCalls.
/// The app/audio side owns actual playback and active sinks; the sim only carries
/// resolved sound ids, stable handles, and positions through the tick event queue.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ModSpatialSoundCommand {
    PlayAt {
        handle: u64,
        sound: crate::audio::Sound,
        pos: crate::mathh::Vec3,
        volume: f32,
        pitch: f32,
    },
    PlayOnMob {
        handle: u64,
        sound: crate::audio::Sound,
        mob_id: u64,
        volume: f32,
        pitch: f32,
        /// The mob position when the command was emitted. If the mob despawns
        /// before the app sees a frame snapshot, playback starts and finishes here.
        last_pos: crate::mathh::Vec3,
    },
    Stop {
        handle: u64,
    },
}

/// One world-anchored event this frame's tick batch carried, in local types —
/// the client-side twin of [`crate::net::protocol::WorldEventMsg`]. Every
/// observer presents these (break bursts, door swings, POSITIONAL sounds);
/// the app maps each to its sound at the event's position.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum WorldEvent {
    BlockBroken {
        pos: IVec3,
        block: Block,
        normal: Option<IVec3>,
    },
    BlockPlaced {
        pos: IVec3,
        block: Block,
    },
    /// A door toggled: the LOWER cell + its NEW open state.
    DoorToggled {
        lower: IVec3,
        open: bool,
    },
    ChestOpened {
        pos: IVec3,
    },
    ChestClosed {
        pos: IVec3,
    },
    /// A player collected a drop at `pos`. `by_self` = the LOCAL player did
    /// (the app keeps its non-positional self pickup sound for that).
    ItemPickedUp {
        pos: Vec3,
        by_self: bool,
    },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GameEvents {
    /// The block placed this frame, if any.
    pub placed_block: Option<Block>,
    /// The block broken (player-mined) this frame, if any.
    pub broke_block: Option<Block>,
    /// The hand swung this frame for an attack.
    pub swung_hand: bool,
    /// An item/stack left the hand for the world this frame.
    pub threw_item: bool,
    /// At least one dropped item was collected into the inventory this frame.
    pub picked_up_item: bool,
    /// The player right-clicked a placed crafting table this frame.
    pub open_crafting_table: bool,
    /// The player right-clicked a placed furnace this frame.
    pub open_furnace: Option<IVec3>,
    /// The player right-clicked a placed chest this frame.
    pub open_chest: Option<IVec3>,
    /// The player right-clicked a placed furniture workbench this frame.
    pub open_furniture_workbench: Option<IVec3>,
    /// A mod GUI should open this frame: from a block's `open_gui` interaction
    /// (`pos = Some`) or a mod's programmatic `GuiOpen` (`pos = None`).
    pub open_mod_gui: Option<(crate::gui::GuiKind, Option<IVec3>)>,
    /// A mod asked to close the open mod GUI this frame (`GuiClose`); the app
    /// honours it only while a mod GUI screen is actually up.
    pub close_mod_gui: bool,
    /// The player right-clicked a door this frame. Carries the door's NEW open
    /// state (after the toggle applied). The open/close SOUND is event-driven
    /// since C2c-iii (the positional [`WorldEvent::DoorToggled`] every
    /// observer receives); this one-shot remains for the toggler's own
    /// presentation. `None` = no door toggle this frame.
    pub toggled_door: Option<bool>,
    /// The player right-clicked a bed this frame. This fires even in daytime,
    /// when the click sets the spawn point but does not start sleep.
    pub bed_interacted: bool,
    /// The player's right-click was CONSUMED by a block interaction this frame
    /// (any `try_open_interactable` capability: container screens, mod GUIs,
    /// doors, beds, a mod cancelling `block_interact`…). Set at the sim's one
    /// decision point, so the interact hand jab is the DEFAULT for every
    /// interaction — present and future — with no per-kind enumeration.
    pub interacted: bool,
    /// The held item's own right-click use fired this frame (a bucket scooping
    /// or pouring water) — plays the same hand jab as placing.
    pub used_item: bool,
    /// The player took damage this frame (post `player_damage_pre`, amount
    /// > 0) — plays the hurt sound and kicks the screen/hand shake.
    pub player_damaged: bool,
    /// The player's health hit 0 this frame — the app opens the death screen.
    pub player_died: bool,
    /// The player right-clicked a bed this frame — the app opens the sleep
    /// overlay.
    pub open_sleep: bool,
    /// The sleep ended this frame (completed, cancelled, or died) — the app
    /// closes the sleep overlay if it is up.
    pub sleep_ended: bool,
    /// The player respawned this frame — the app closes the death screen.
    pub respawned: bool,
    /// Every sound mods emitted across this frame's fixed ticks, in emission
    /// order. NON-lossy (unlike the latched booleans above): each entry plays
    /// exactly once.
    pub mod_sounds: Vec<ModSound>,
    /// Spatial sound start/stop commands emitted by mods across this frame's
    /// fixed ticks. NON-lossy; the app/audio side owns active playback state.
    pub mod_spatial_sounds: Vec<ModSpatialSoundCommand>,
    /// Semantic mob sound events emitted by gameplay across this frame's fixed
    /// ticks. NON-lossy; the app resolves species data and plays them.
    pub mob_sounds: Vec<MobSoundEvent>,
    /// World-anchored events every observer presents (positional sounds,
    /// break bursts, door swings), in emission order. NON-lossy.
    pub world_events: Vec<WorldEvent>,
    /// The server became unreachable (thread crashed / channel closed) —
    /// reported EXACTLY ONCE, on the frame the loss is detected. The app
    /// grows a proper "world stopped" screen for it in Phase E; until then it
    /// is logged and the (frozen) world keeps rendering.
    pub connection_lost: Option<String>,
}

/// The per-PLAYER slice of what the tick did: the lossy latched one-shots that
/// feed that player's [`GameEvents`] (hand jabs, hurt shake, screen requests).
/// One per session per tick; the acting session's slice is written by the
/// per-player stages.
#[derive(Copy, Clone, Debug, Default)]
pub(crate) struct PlayerTickEvents {
    pub(crate) broke_block: Option<Block>,
    pub(crate) placed_block: Option<Block>,
    pub(crate) swung_hand: bool,
    pub(crate) picked_up_item: bool,
    pub(crate) threw_item: bool,
    pub(crate) used_item: bool,
    /// An eat COMPLETED this tick (the food was consumed) — as opposed to the
    /// level `eating` state ending in an abort. Feeds the remote-player
    /// `AteFinished` action; the local client's presentation reads the eat
    /// progress instead.
    pub(crate) ate_finished: bool,
    pub(crate) bed_interacted: bool,
    pub(crate) interacted: bool,
    pub(crate) player_damaged: bool,
    pub(crate) player_died: bool,
    pub(crate) sleep_ended: bool,
    pub(crate) respawned: bool,
    /// The door toggle's NEW open state, latched for the TOGGLER only.
    pub(crate) toggled_door: Option<bool>,
}

/// A block the sim destroyed this tick (player-mined or natural), with
/// everything a CLIENT needs to present it: break-burst particles at `pos`,
/// sampled against the post-tick world. Position-carrying and broadcastable —
/// the wire replicates these to every client in range (multiplayer Phase C).
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct BlockBrokenEvent {
    pub(crate) pos: IVec3,
    pub(crate) block: Block,
    /// The mined face (for directional burst spread), when known.
    pub(crate) normal: Option<IVec3>,
}

/// The WORLD-anchored slice of what the tick did: non-lossy queues every
/// observer cares about, independent of which player acted. `sounds`/
/// `spatial_sounds`/`mob_sounds` are the existing presentation feeds;
/// `block_broken`/`door_changed` are consumed client-side
/// after the tick (particles, swing/lid animation seeds) and become broadcast
/// messages when the wire exists.
#[derive(Clone, Debug)]
pub(crate) struct WorldEvents {
    pub(crate) sounds: Vec<ModSound>,
    pub(crate) spatial_sounds: Vec<ModSpatialSoundCommand>,
    pub(crate) mob_sounds: Vec<MobSoundEvent>,
    pub(crate) block_broken: Vec<BlockBrokenEvent>,
    /// A block placed by a player: (anchor cell, block).
    pub(crate) block_placed: Vec<(IVec3, Block)>,
    /// A door toggled: (lower cell, new open state).
    pub(crate) door_changed: Vec<(IVec3, bool)>,
    /// A chest's viewer count crossed 0↔1: (chest cell, now open).
    pub(crate) chest_changed: Vec<(IVec3, bool)>,
    /// A player collected at least one drop: (their body centre, player id).
    pub(crate) item_picked_up: Vec<(Vec3, PlayerId)>,
    next_spatial_sound_handle: u64,
}

impl WorldEvents {
    fn with_next_spatial_sound_handle(next_spatial_sound_handle: u64) -> Self {
        Self {
            sounds: Vec::new(),
            spatial_sounds: Vec::new(),
            mob_sounds: Vec::new(),
            block_broken: Vec::new(),
            block_placed: Vec::new(),
            door_changed: Vec::new(),
            chest_changed: Vec::new(),
            item_picked_up: Vec::new(),
            next_spatial_sound_handle: next_spatial_sound_handle.max(1),
        }
    }
}

/// What the world-mutating actions did across the fixed tick(s) that ran this frame.
/// The tick→presentation channel: the event bus feeds it (via `SimCtx::feed`),
/// never the other way around. Crate-visible so event handlers can write it.
/// Split per audience: `players[s]` is player `s`'s lossy one-shot slice,
/// `world` the shared non-lossy queues.
#[derive(Clone, Debug)]
pub(crate) struct TickEvents {
    players: Vec<PlayerTickEvents>,
    pub(crate) world: WorldEvents,
}

impl Default for TickEvents {
    fn default() -> Self {
        Self::with_next_spatial_sound_handle(1)
    }
}

impl TickEvents {
    pub(crate) fn with_next_spatial_sound_handle(next_spatial_sound_handle: u64) -> Self {
        Self {
            players: Vec::new(),
            world: WorldEvents::with_next_spatial_sound_handle(next_spatial_sound_handle),
        }
    }

    /// Player `s`'s event slice, grown on demand so tests (and late joins mid-
    /// frame) never index out of bounds.
    pub(crate) fn player(&mut self, s: usize) -> &mut PlayerTickEvents {
        if self.players.len() <= s {
            self.players.resize_with(s + 1, Default::default);
        }
        &mut self.players[s]
    }

    /// Read-only copy of player `s`'s slice (default if nothing was written).
    pub(crate) fn player_at(&self, s: usize) -> PlayerTickEvents {
        self.players.get(s).copied().unwrap_or_default()
    }

    pub(crate) fn next_spatial_sound_handle(&self) -> u64 {
        self.world.next_spatial_sound_handle
    }

    pub(crate) fn alloc_spatial_sound_handle(&mut self) -> u64 {
        let handle = self.world.next_spatial_sound_handle.max(1);
        self.world.next_spatial_sound_handle = handle.wrapping_add(1).max(1);
        handle
    }
}

/// Client-side tick clock: `tick_alpha` used to read the server accumulator,
/// which now lives on the server thread. Instead, note the arrival `Instant`
/// of each applied `TickUpdate` and measure into the current tick from there.
/// Simple and monotonic per interval; thread-scheduling jitter shows up as
/// small alpha noise — smoothing (e.g. an EMA over update spacing) can layer
/// on later if interpolation visibly judders.
#[derive(Default)]
pub(crate) struct ReplicaClock {
    last_update: Option<std::time::Instant>,
}

impl ReplicaClock {
    /// Note that a `TickUpdate` was just applied.
    pub(crate) fn note_update(&mut self) {
        self.last_update = Some(std::time::Instant::now());
    }

    /// Fraction (0..1) into the current fixed tick since the last update.
    /// `1.0` before the first update (render current state, no interpolation)
    /// and while updates stall (pause, hitches) — poses hold rather than snap.
    pub(crate) fn alpha(&self) -> f32 {
        match self.last_update {
            Some(at) => (at.elapsed().as_secs_f32() / TICK_DT).clamp(0.0, 1.0),
            None => 1.0,
        }
    }
}

/// The client-side accumulation of one frame's `TickUpdate` event payloads,
/// already translated to LOCAL types (ids remapped at the transport for a
/// remote client; identity in-process). Filled by `apply_tick_update`, drained
/// once per frame by [`Game::tick`] into `GameEvents`.
#[derive(Default)]
pub(crate) struct ClientEvents {
    pub(crate) world: Vec<WorldEvent>,
    pub(crate) self_events: SelfEvents,
    pub(crate) mod_sounds: Vec<ModSound>,
    pub(crate) mod_spatial_sounds: Vec<ModSpatialSoundCommand>,
    pub(crate) mob_sounds: Vec<MobSoundEvent>,
}

impl Game {
    pub fn tick(&mut self, dt: f32, input: &GameInput) -> GameEvents {
        self.tick_send(dt, input);
        self.tick_receive(dt)
    }

    /// The frame's INPUT half: per-frame client systems, then this frame's
    /// protocol messages onto the server channel — the ONLY road input takes
    /// into the sim (the server thread latches them before its next ticks).
    /// Split from [`tick_receive`](Self::tick_receive) so the test harness can
    /// service the pipe synchronously between the halves (production runs
    /// them back-to-back; the thread answers asynchronously).
    pub(crate) fn tick_send(&mut self, dt: f32, input: &GameInput) {
        // Per-frame exceptions kept for local feel: look, hotbar, local player, entity push.
        self.apply_camera_input(input);
        self.apply_hotbar_input(input);
        self.tick_player(dt, input);
        self.apply_entity_push(dt);
        self.tick_replica_view();
        self.refresh_target();
        self.update_third_person(dt);
        self.tick_local_mining(dt, input);

        let update = self.build_player_update(input);
        self.build_outgoing_messages(input, update);
        // What we are claiming this frame — the reference a later
        // `SelfState::transform` correction diffs against (fields the server
        // merely echoes back must not stomp newer local values).
        self.last_sent_transform = Some(crate::net::protocol::SelfTransform {
            pos: update.pos,
            vel: update.vel,
            yaw: update.yaw,
            pitch: update.pitch,
            on_ground: update.on_ground,
        });
        let mut lost = false;
        for msg in self.frame_messages.drain(..) {
            if lost {
                continue; // drain the rest; the server is gone
            }
            lost = self.handle.send(msg).is_err();
        }
        if lost {
            self.note_connection_lost();
        }
    }

    /// The frame's OUTPUT half: drain + apply the server's messages, then the
    /// per-frame presentation systems, and assemble the app-facing events.
    pub(crate) fn tick_receive(&mut self, dt: f32) -> GameEvents {
        // Apply the drained messages (terrain installs, then tick batches)
        // BEFORE any presentation/HUD read — the same messages a remote
        // client applies off the wire. Everything below consumes ONLY what
        // those messages carried (`ClientEvents`), never sim state. More than
        // one `TickUpdate` may have landed since last frame; `ClientEvents`
        // accumulates them (one-shots OR, queues append, latest state wins).
        self.pump_network();

        // Presentation/infra after fixed simulation; no gameplay mutation here.
        // Remote players' per-frame animation state (shared body pose,
        // held-item animator, hurt/eat ramps) advances right after the batches
        // applied, so this frame's latched one-shots jab this frame.
        let alpha = self.replica_clock.alpha();
        self.remote_players.advance(dt, alpha);
        let events = std::mem::take(&mut self.pending_events);
        self.sync_sleep_camera_on_open(&events.self_events);
        // World-anchored effects the tick batch carried (break bursts, door
        // swings) spawn from the replicated events — the identical path a
        // remote client drives them from off the wire.
        self.apply_world_effects(&events.world);
        self.tick_mining_dust(dt);
        self.tick_entities(dt);
        self.advance_chest_lids(dt);
        self.advance_door_swings(dt);
        self.tick_mesh_budget();

        self.assemble_game_events(events)
    }

    /// Drain and apply every pending server→client message. `Game::tick` runs
    /// it every frame; the App ALSO calls it while a shell screen (pause menu)
    /// suppresses `Game::tick`, so the client keeps consuming server output —
    /// streaming installs land, the channel never backs up, and resume is
    /// instant. Also where a dead server (crash / closed channel) is detected.
    pub fn pump_network(&mut self) {
        if self.handle.is_crashed() {
            self.note_connection_lost();
        }
        let mut msgs = std::mem::take(&mut self.incoming);
        self.handle.drain(&mut msgs);
        self.apply_server_messages(&mut msgs);
        self.incoming = msgs; // drained; capacity reused
    }

    /// Advance the local mining timer for crack overlay and emit
    /// `BreakFinished` when the client finishes a break.
    fn tick_local_mining(&mut self, dt: f32, input: &GameInput) {
        let tool = self
            .self_view
            .inventory
            .selected()
            .and_then(|st| st.item.tool());
        let look = self.look.map(|h| h.block);
        let inventory_open = !input.gameplay_enabled;
        let event = self.local_mining.update(
            dt,
            look,
            input.break_held && input.gameplay_enabled,
            inventory_open,
            &self.replica,
            tool,
        );
        // Prefer local crack over the lagged SelfView copy.
        self.self_view.mining = self.local_mining.overlay().or(self.self_view.mining);

        if let Some(ev) = event {
            // No duration claim rides the wire: the server validates the
            // finish against ITS OWN observed mining window (breaking.rs).
            let request_id = self.prediction.begin_track_only();
            let tool_item_id = self
                .self_view
                .inventory
                .selected()
                .map(|st| st.item.0);
            self.outbox
                .push(ClientToServer::Action(PlayerAction::BreakFinished {
                    request_id,
                    pos: ev.pos,
                    tool_item_id,
                }));
        }
    }

    /// Map this frame's replicated event payloads onto the app-facing
    /// `GameEvents` shape (the app's consumption is unchanged: the one-shots
    /// and `open_*` fields read exactly as they did pre-wire).
    fn assemble_game_events(&mut self, events: ClientEvents) -> GameEvents {
        let se = events.self_events;
        let local_jab = std::mem::take(&mut self.local_hand_jab);
        let local_swing = std::mem::take(&mut self.local_hand_swing);
        let mut out = GameEvents {
            placed_block: se.placed_block.map(Block::from_id),
            broke_block: se.broke_block.map(Block::from_id),
            swung_hand: se.swung_hand || local_swing,
            picked_up_item: se.picked_up_item,
            threw_item: se.threw_item,
            close_mod_gui: se.close_mod_gui,
            toggled_door: se.toggled_door,
            bed_interacted: se.bed_interacted,
            interacted: se.interacted || local_jab,
            used_item: se.used_item,
            player_damaged: se.player_damaged,
            player_died: se.player_died,
            sleep_ended: se.sleep_ended,
            respawned: se.respawned,
            mod_sounds: events.mod_sounds,
            mod_spatial_sounds: events.mod_spatial_sounds,
            mob_sounds: events.mob_sounds,
            world_events: events.world,
            ..Default::default()
        };
        if !self.connection_lost_reported {
            if let Some(reason) = &self.connection_lost {
                log::error!("{reason}; nothing further will be saved");
                out.connection_lost = Some(reason.clone());
                self.connection_lost_reported = true;
            }
        }
        match se.open_screen {
            // The client itself requested the inventory (the E key already
            // opened its screen); the event is the server's ack.
            Some(OpenScreen::Inventory) | None => {}
            Some(OpenScreen::CraftingTable) => out.open_crafting_table = true,
            Some(OpenScreen::Furnace(pos)) => out.open_furnace = Some(pos),
            Some(OpenScreen::Chest(pos)) => out.open_chest = Some(pos),
            // Only presence is meaningful — the workbench session carries no
            // position (the field keeps its historical Option<IVec3> shape).
            Some(OpenScreen::Workbench) => out.open_furniture_workbench = Some(IVec3::ZERO),
            Some(OpenScreen::ModGui { kind_key, pos }) => {
                // Unknown kind = a mod the client lacks; the handshake makes
                // this unreachable in practice — skip rather than panic.
                if let Some(kind) = crate::gui::resolve_kind(&kind_key) {
                    out.open_mod_gui = Some((kind, pos));
                }
            }
            Some(OpenScreen::Sleep) => out.open_sleep = true,
        }
        out
    }

    /// This frame's transform + held-intent message, built from the predicted
    /// player and the per-frame targeting. Held intents ride raw — the server
    /// forces them off while `gameplay` is false (the old `capture_intent`
    /// rule); the held-rotation counter rides raw and the session re-derives
    /// the armed item (see `HeldRotation::apply_wire`).
    fn build_player_update(&self, input: &GameInput) -> PlayerUpdate {
        // The movement intent is EXACTLY what this frame's local physics
        // consumed (`tick_player` stashes it) — re-deriving it here would read
        // the camera after `sync_camera_to_player_eye` moved it and drift the
        // wire intent from the prediction.
        let intent = self.predicted_input;
        PlayerUpdate {
            pos: self.player.pos,
            vel: self.player.vel,
            yaw: self.player.yaw,
            pitch: self.player.pitch,
            on_ground: self.player.on_ground,
            sneak: input.movement.sneak,
            gameplay: input.gameplay_enabled,
            break_held: input.break_held,
            use_held: input.use_held,
            target: self.look.map(|h| TargetRef {
                block: h.block,
                normal: h.normal,
            }),
            hotbar_slot: self.player.inventory.active_slot(),
            held_rotation: self.held_rotation.rotation,
            wishdir: intent.wishdir,
            jump: intent.jump,
            sprint: intent.sprint,
        }
    }

    /// Assemble this frame's message batch into `frame_messages`, in
    /// consumption order: the `PlayerUpdate` first (so the edge-drop rule and
    /// slot-dependent actions see this frame's state), then this frame's click
    /// edges (mob targets resolved to STABLE ids now, at click time), then
    /// everything the app-facing methods queued since the last frame.
    fn build_outgoing_messages(&mut self, input: &GameInput, update: PlayerUpdate) {
        debug_assert!(self.frame_messages.is_empty(), "pump drains every frame");
        let use_mob = input
            .place_clicked
            .then(|| self.targeted_mob_id())
            .flatten();
        let attack_mob = input
            .attack_clicked
            .then(|| self.targeted_mob_id())
            .flatten();
        // At most one of mob/player is targeted per frame (refresh_target's
        // nearest-wins pick), so the click carries at most one.
        let attack_player = input
            .attack_clicked
            .then_some(self.targeted_player)
            .flatten();
        self.frame_messages
            .push(ClientToServer::PlayerUpdate(update));
        if input.gameplay_enabled {
            if input.place_clicked {
                // P0: jab immediately; optional place ghost (P1) when holding a block.
                self.local_hand_jab = true;
                let request_id = self.try_predict_place_ghost(input.movement.sneak);
                self.frame_messages
                    .push(ClientToServer::Action(PlayerAction::UseClick {
                        mob: use_mob,
                        request_id,
                    }));
            }
            if input.attack_clicked {
                self.local_hand_swing = true;
                self.frame_messages
                    .push(ClientToServer::Action(PlayerAction::AttackClick {
                        mob: attack_mob,
                        player: attack_player,
                    }));
            }
        }
        self.frame_messages.append(&mut self.outbox);
    }

    /// Optimistic ghost place when the look target can accept the held block.
    fn try_predict_place_ghost(
        &mut self,
        sneak: bool,
    ) -> Option<crate::net::protocol::ClientRequestId> {
        let look = self.look?;
        let block = self.self_view.inventory.selected()?.item.as_block()?;
        // A non-sneak click on an interactable block opens/uses it instead of
        // placing (the server's interact ladder) — no ghost, or the client
        // would render a phantom block the server never places.
        let target = crate::block::Block::from_id(self.replica.chunk_block(
            look.block.x,
            look.block.y,
            look.block.z,
        ));
        if !sneak && target.interaction() != crate::block::BlockInteraction::None {
            return None;
        }
        let place_pos = look.block + look.normal;
        let prev = self.replica.chunk_block(place_pos.x, place_pos.y, place_pos.z);
        if prev != crate::block::Block::Air.0 {
            return None;
        }
        if !self.prediction.can_predict() {
            return Some(self.prediction.begin_track_only());
        }
        let snapshot = prediction::PredictionSnapshot::Cell {
            pos: place_pos,
            prev_block_id: prev,
        };
        let id = self.prediction.begin(snapshot);
        let _ = self
            .replica
            .set_block_world(place_pos.x, place_pos.y, place_pos.z, block);
        self.place_ghost = Some((place_pos, block.0));
        Some(id)
    }

    /// Adopt a `SelfState::transform` correction: the server's ticks moved
    /// this player (bed tuck, wake/respawn teleports, mod `Teleport`,
    /// mob-strike knockback). Per-field against the transform we last SENT:
    /// a field still equal to our last claim is the server echoing us — the
    /// local value (possibly a frame newer: look, movement) wins; a differing
    /// field is a genuine server-side mutation. A position change adopts via
    /// `Player::teleport` so the client's own fall bookkeeping re-anchors too.
    /// Without a `last_sent_transform` (before the first frame) everything
    /// adopts — the values are the shared restore, so it is a no-op.
    pub(crate) fn adopt_authoritative_transform(
        &mut self,
        t: &crate::net::protocol::SelfTransform,
    ) {
        let sent = self.last_sent_transform;
        if sent.is_none_or(|s| s.pos != t.pos) {
            self.player.teleport(t.pos);
        }
        if sent.is_none_or(|s| s.vel != t.vel) {
            self.player.vel = t.vel;
        }
        if sent.is_none_or(|s| s.yaw != t.yaw) {
            self.player.yaw = t.yaw;
        }
        if sent.is_none_or(|s| s.pitch != t.pitch) {
            self.player.pitch = t.pitch;
        }
        if sent.is_none_or(|s| s.on_ground != t.on_ground) {
            self.player.on_ground = t.on_ground;
        }
    }
}
