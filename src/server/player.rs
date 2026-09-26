//! Per-connected-player simulation state.
//!
//! Everything the sim tracks *per player* lives here — the fields that used to
//! sit directly on `Game` when the game was single-player. The tick stages
//! (`src/game/*.rs`) loop the sessions in id order; the client-facing session
//! is always index 0 until remote connections exist.
//!
//! Input reaches a session only through `net::protocol`
//! messages (`ServerGame::apply_message`): `PlayerUpdate` latches the
//! transform, targeting, and held intents; `PlayerAction`/menu actions latch the
//! one-shot edges. The tick stages consume the latches exactly as before.

use crate::menu::ContainerMenu;
use crate::net::protocol::TargetRef;
use crate::player::Player;
use crate::server::bed::SleepState;
use crate::server::drops::DropQueue;
use crate::server::item_use::EatingState;
use petramond_math::math::IVec3;
use petramond_world::item::ItemType;
use petramond_world::mining::MiningState;

mod latches;

pub use crate::player::PlayerId;
pub use latches::{
    AttackClick, InputLatches, PendingBreakFinished, PendingMenuAction, PendingUseClick,
    BREAK_QUEUE_DEPTH, MENU_ACTIONS_PER_TICK, MENU_QUEUE_DEPTH,
};

pub use crate::world::placement_types::HeldRotation;

/// Server-side fall measurement from the per-tick transform samples of
/// `tick_movement` — the replicated-transform mirror of `Player::track_fall`
/// (the client physics still measures its own falls, but the server no longer
/// reads that latch). Water re-anchors the peak (water breaks a fall); an
/// airborne→grounded transition measures the landing; while airborne the peak
/// tracks the highest reported point. Because it samples only once per tick,
/// `tick_movement` also feeds it the server integration's own ground contacts:
/// a sprint down stairs touches each step for less than a sample interval,
/// and without those contacts the staircase would measure as one tall fall.
#[derive(Clone, Debug)]
pub struct FallTracker {
    peak_y: f64,
    airborne: bool,
}

impl FallTracker {
    pub fn new(y: f64) -> Self {
        Self {
            peak_y: y,
            airborne: false,
        }
    }

    /// Re-anchor at `y` and drop any airborne state — teleports and mode
    /// switches are never falls (mirrors `Player::teleport`/`set_mode`).
    pub fn reset(&mut self, y: f64) {
        self.peak_y = y;
        self.airborne = false;
    }

    /// Feed one reported transform. Returns what the update concluded: a dry
    /// landing (airborne → grounded) with its fall distance, or a water entry
    /// (airborne → in water) with the distance fallen into the surface —
    /// walking into water arrives grounded/level and reports nothing.
    pub fn observe(&mut self, y: f64, on_ground: bool, in_water: bool) -> Option<FallOutcome> {
        if in_water {
            let was_airborne = self.airborne;
            let dist = (self.peak_y - y) as f32;
            self.peak_y = y;
            self.airborne = !on_ground;
            return (was_airborne && dist > 0.0).then_some(FallOutcome::Splashed(dist));
        }
        if on_ground {
            let landed = self.airborne;
            let dist = (self.peak_y - y) as f32;
            self.peak_y = y;
            self.airborne = false;
            return (landed && dist > 0.0).then_some(FallOutcome::Landed(dist));
        }
        self.peak_y = self.peak_y.max(y);
        self.airborne = true;
        None
    }
}

/// What one [`FallTracker::observe`] concluded, with the fall distance in blocks.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum FallOutcome {
    /// Airborne → grounded, dry: fall damage applies.
    Landed(f32),
    /// Airborne → in water: no damage (water breaks the fall), but a hard
    /// enough entry throws the water-splash burst.
    Splashed(f32),
}

/// One player's simulation session: the authoritative player plus every
/// per-player latch, timer, and menu session the tick stages consume,
/// grouped by who owns it — the sim ([`SessionSim`]), the latched client
/// input ([`InputLatches`]), per-recipient replication ([`SessionReplication`])
/// and the connection's streaming state ([`SessionTransport`]). Every field
/// is private to the server; the rest of the engine reads a session through
/// the accessors below.
pub struct ConnectedPlayer {
    pub(in crate::server) id: PlayerId,
    /// The player's authenticated identity: keys the save file
    /// (`players/<key>.dat`) and operator rights. The local session's is the
    /// host client's own identity.
    pub(in crate::server) key: crate::net::identity::PlayerKey,
    /// Display name, unique among the world's identities
    /// (`server::accounts`). Never a save or permission key.
    pub(in crate::server) name: String,
    /// The authoritative player body.
    pub(in crate::server) player: Player,
    pub(in crate::server) sim: SessionSim,
    pub(in crate::server) input: InputLatches,
    pub(in crate::server) replication: SessionReplication,
    pub(in crate::server) transport: SessionTransport,
}

/// Per-session simulation state beyond the body: mining and editing
/// progress, timers, the open menu, sleep, the riding mirror.
pub struct SessionSim {
    pub mining: MiningState,
    /// When this session last broke a block at once, without mining it: the
    /// repeat gate that keeps a held break from tearing through a row.
    pub last_instant_break: Option<u64>,
    pub creative: super::creative::CreativeSession,
    /// Undo/redo over every edit this session makes while it edits cells.
    pub edits: super::creative::EditHistory,
    /// Schematic choices, positionings, archive streams and ghosts shared
    /// with this client.
    pub schematic: super::schematics::SchematicSession,
    pub attack_cooldown: u32,
    /// Server-side fall measurement from the reported transforms.
    pub fall: FallTracker,
    /// Hardest landing (blocks) since the tick last consumed it, measured by
    /// [`fall`](Self::fall) — `tick_fall_damage` converts it into damage.
    pub pending_fall: f32,
    /// Hardest fall INTO a splashing fluid (blocks) since the tick last
    /// consumed it — `tick_fluid_splash` converts it into that fluid's splash.
    pub pending_splash: f32,
    /// The in-progress eat (held secondary button on food), or `None`.
    pub eating: Option<EatingState>,
    pub drop_queue: DropQueue,
    /// SESSION MIRROR of this player's entry in the world riding registry,
    /// maintained by the riding pass (`server::riding`) — the mirror drives
    /// physical placement on a detach and what per-session consumers read
    /// (movement skip, replication row). Detach events are recorded at the
    /// authoritative registry transition, not inferred from this mirror.
    pub mount: Option<crate::mob::riding::Mount>,
    /// The open container GUI's persistent edit target for THIS player.
    pub menu: ContainerMenu,
    /// The in-flight sleep session (`None` = awake).
    pub sleep: Option<SleepState>,
    /// The open mod-GUI session's state map (written by mods on the tick via
    /// `GuiStateSet`, cleared by the menu funnels on open/close). Snapshotted
    /// behind the `Arc` per replication batch — copy-on-write on writes.
    pub gui_state: std::sync::Arc<petramond_world::gui_state::GuiStateMap>,
}

/// Per-recipient replication: the one-shot outbox the tick fills for this
/// client and the bookkeeping that decides what its next batch carries.
/// Replication state, not sim state.
pub struct SessionReplication {
    /// Cells whose CURRENT authoritative state ships to this recipient in the
    /// next batch (a use click that resolved to nothing, a denied place, or a
    /// denied break): the reconcile channel for a client whose replica disagreed.
    pub pending_corrective_cells: Vec<IVec3>,
    /// Cells this session placed this tick window — stripped from the
    /// initiator's `TickUpdate.events` so their local place prediction does
    /// not hear a second `BlockPlaced` one RTT later. Taken in
    /// `build_tick_update`.
    pub presented_places: Vec<IVec3>,
    /// Cells this session broke this tick window — same echo filter for
    /// `BlockBroken`. Taken in `build_tick_update`.
    pub presented_breaks: Vec<IVec3>,
    /// Outcomes queued this tick window for the next `TickUpdate`.
    pub pending_action_outcomes: Vec<crate::net::protocol::ActionOutcome>,
    /// World events addressed to THIS recipient only, shipped ahead of the
    /// shared list in its next tick batch — the join catch-up for stateful
    /// world presentation (the spatial loops still playing).
    pub pending_world_events: Vec<crate::net::protocol::WorldEventMsg>,
    /// Hand-swing one-shots latched by this tick's action stages (attack,
    /// break, place, throw) via [`ConnectedPlayer::latch_swing`], published
    /// on the next roster and cleared — the swing facts behind the mod ABI's
    /// `PlayerSnapshot::swing`.
    pub swing_events: mod_api::HandSwing,
    /// The GUI session the tick opened for this client this tick, if any —
    /// one field for every kind (engine containers and mod GUIs alike).
    pub request_open_gui: Option<(
        petramond_world::gui_state::GuiKind,
        Option<crate::menu::MenuAnchor>,
    )>,
    pub request_close_gui: bool,
    pub request_open_sleep: bool,
    /// Player position when this frame's fixed ticks began — a tick-side
    /// position change is a teleport, which re-anchors the fall tracker (see
    /// `ServerGame::pump`).
    pub pos_before_ticks: petramond_math::world_pos::WorldPos,
    /// Whether the LAST tick window teleported this player (the drift check
    /// over [`pos_before_ticks`](Self::pos_before_ticks)) — replicated as
    /// `PlayerStateRow::snap` so observers skip interpolating across the
    /// jump. Refreshed every pump.
    pub tick_teleported: bool,
    /// The inventory revision the last emitted `SelfState` carried a full
    /// inventory for. `None` = nothing sent yet, so the first update after
    /// join always includes the inventory.
    pub last_sent_inventory_revision: Option<u64>,
    /// The inventory revision the obtained-item scan last ran against, so a
    /// tick that changed nothing costs one comparison (see
    /// `server::progression`).
    pub last_obtained_scan: Option<u64>,
    /// How many of this player's unlocked recipes the client has been told
    /// about. Unlocking only appends, so the catch-up is the untold suffix.
    pub sent_unlock_count: usize,
    /// The last `MenuSyncMsg` this session was sent (its `gui_state` field
    /// always `None` — the map compares by `Arc` identity below). On-change
    /// send detection.
    pub last_menu_sync: Option<crate::net::protocol::MenuSyncMsg>,
    /// The `gui_state` map allocation the last sync shipped. Holding the
    /// `Arc` is what makes identity comparison sound: the next tick-side
    /// write is forced to copy-on-write onto a fresh allocation.
    pub last_sent_gui_state:
        Option<std::sync::Arc<petramond_world::gui_state::GuiStateMap>>,
    /// The transform of the last `PlayerUpdate` this session applied — what
    /// the CLIENT last claimed. After the ticks, a session transform that no
    /// longer matches it means the tick moved the player (teleport,
    /// knockback): the next `SelfState` ships a [`SelfTransform`] correction.
    ///
    /// [`SelfTransform`]: crate::net::protocol::SelfTransform
    pub last_reported_transform: Option<crate::net::protocol::SelfTransform>,
}

/// The connection's streaming state: which terrain and entities this client
/// holds, and how far it asked to see.
pub struct SessionTransport {
    /// Per-connection terrain replication state (which columns/sections this
    /// client holds) — see `server::streaming`.
    pub terrain: crate::server::streaming::TerrainSync,
    /// Per-connection entity interest (which mobs, items and players this
    /// client tracks) — see `server::game::interest`.
    pub interest: crate::server::game::EntityInterest,
    /// This client's REQUESTED view distance in chunks (`Join` /
    /// `SetViewDistance`, clamped `4..=64`). Streaming uses
    /// `min(this, world.data().render_dist)` — the server's own budget stays the
    /// ceiling, per-connection requests only shrink it.
    pub view_radius: i32,
}

impl SessionReplication {
    /// Ship the full inventory with the next batch even if its revision did
    /// not move: a predicted menu action must reconcile from exactly that
    /// batch, whatever the on-change gate thinks.
    pub fn force_inventory_resync(&mut self) {
        self.last_sent_inventory_revision = None;
    }

    /// Ship the menu sync with the next batch even if it compares equal.
    pub fn force_menu_resync(&mut self) {
        self.last_menu_sync = None;
    }

    /// Ship the open GUI's state map with the next menu sync.
    pub fn force_gui_state_resync(&mut self) {
        self.last_sent_gui_state = None;
    }

    /// Queue `outcome` for this recipient's next batch.
    pub fn push_outcome(&mut self, outcome: crate::net::protocol::ActionOutcome) {
        self.pending_action_outcomes.push(outcome);
    }
}

impl ConnectedPlayer {
    pub fn new(
        id: PlayerId,
        key: crate::net::identity::PlayerKey,
        name: String,
        player: Player,
        view_radius: i32,
    ) -> Self {
        let fall = FallTracker::new(player.pos.y);
        let pos_before_ticks = player.pos;
        Self {
            id,
            key,
            name,
            sim: SessionSim {
                mining: MiningState::new(),
                last_instant_break: None,
                creative: Default::default(),
                edits: Default::default(),
                schematic: Default::default(),
                attack_cooldown: 0,
                fall,
                pending_fall: 0.0,
                pending_splash: 0.0,
                eating: None,
                drop_queue: DropQueue::default(),
                mount: None,
                menu: ContainerMenu::new(),
                sleep: None,
                gui_state: petramond_world::gui_state::empty_gui_state(),
            },
            input: InputLatches::new(pos_before_ticks),
            replication: SessionReplication {
                pending_corrective_cells: Vec::new(),
                presented_places: Vec::new(),
                presented_breaks: Vec::new(),
                pending_action_outcomes: Vec::new(),
                pending_world_events: Vec::new(),
                swing_events: Default::default(),
                request_open_gui: None,
                request_close_gui: false,
                request_open_sleep: false,
                pos_before_ticks,
                tick_teleported: false,
                last_sent_inventory_revision: None,
                last_obtained_scan: None,
                sent_unlock_count: 0,
                last_menu_sync: None,
                last_sent_gui_state: None,
                last_reported_transform: None,
            },
            transport: SessionTransport {
                terrain: Default::default(),
                interest: Default::default(),
                view_radius: view_radius.clamp(4, 64),
            },
            player,
        }
    }

    /// The session's stable id.
    #[inline]
    pub fn id(&self) -> PlayerId {
        self.id
    }

    /// The session's authenticated identity.
    #[inline]
    pub fn key(&self) -> crate::net::identity::PlayerKey {
        self.key
    }

    /// The session's display name.
    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The authoritative player body.
    #[inline]
    pub fn player(&self) -> &Player {
        &self.player
    }

    /// The authoritative player body, for fixtures that stage player state.
    #[cfg(any(test, feature = "test-support"))]
    pub fn player_mut(&mut self) -> &mut Player {
        &mut self.player
    }

    /// The riding mirror (see [`SessionSim::mount`]).
    #[inline]
    pub fn mount(&self) -> Option<crate::mob::riding::Mount> {
        self.sim.mount
    }

    /// The open container session.
    #[inline]
    pub fn menu(&self) -> &ContainerMenu {
        &self.sim.menu
    }

    /// The reach-validated look target latched from the last update.
    #[inline]
    pub fn look(&self) -> Option<TargetRef> {
        self.input.look
    }

    /// The open mod GUI's state map.
    #[inline]
    pub fn gui_state(&self) -> &std::sync::Arc<petramond_world::gui_state::GuiStateMap> {
        &self.sim.gui_state
    }

    /// Fixture read of the simulation group.
    #[cfg(any(test, feature = "test-support"))]
    pub fn sim(&self) -> &SessionSim {
        &self.sim
    }

    /// Fixture access to the simulation group.
    #[cfg(any(test, feature = "test-support"))]
    pub fn sim_mut(&mut self) -> &mut SessionSim {
        &mut self.sim
    }

    /// Fixture read of the latched input.
    #[cfg(any(test, feature = "test-support"))]
    pub fn input(&self) -> &InputLatches {
        &self.input
    }

    /// Fixture access to the latched input.
    #[cfg(any(test, feature = "test-support"))]
    pub fn input_mut(&mut self) -> &mut InputLatches {
        &mut self.input
    }

    /// Fixture read of the replication bookkeeping.
    #[cfg(any(test, feature = "test-support"))]
    pub fn replication(&self) -> &SessionReplication {
        &self.replication
    }

    /// Fixture access to the replication bookkeeping.
    #[cfg(any(test, feature = "test-support"))]
    pub fn replication_mut(&mut self) -> &mut SessionReplication {
        &mut self.replication
    }

    /// Fixture read of the streaming state.
    #[cfg(any(test, feature = "test-support"))]
    pub fn transport(&self) -> &SessionTransport {
        &self.transport
    }

    /// Fixture access to the streaming state.
    #[cfg(any(test, feature = "test-support"))]
    pub fn transport_mut(&mut self) -> &mut SessionTransport {
        &mut self.transport
    }

    #[inline]
    pub fn selected_item(&self) -> Option<ItemType> {
        self.player.inventory.selected().map(|s| s.item)
    }

    /// Latch one hand's swing one-shot for the next roster publish (see
    /// [`SessionReplication::swing_events`]). Newest wins within a hand.
    pub fn latch_swing(
        &mut self,
        hand: petramond_world::inventory::Hand,
        kind: mod_api::SwingKind,
    ) {
        let swing = &mut self.replication.swing_events;
        match hand {
            petramond_world::inventory::Hand::Main => swing.main = Some(kind),
            petramond_world::inventory::Hand::Off => swing.off = Some(kind),
        }
    }

    /// Whether this session is sneaking RIGHT NOW: the held sneak intent,
    /// gated on gameplay focus like every other held intent (a menu releases
    /// it). The single definition every consumer shares — movement physics,
    /// the interact-vs-place gate, the replicated `sneaking` row, and the mob
    /// AI's player anchor.
    #[inline]
    pub fn sneaking(&self) -> bool {
        self.input.intent_sneak && self.input.intent_gameplay
    }

    /// Whether this session is HOLDING the interact (use) button right now:
    /// the held use intent, gated on gameplay focus like
    /// [`sneaking`](Self::sneaking). The one definition published to mods
    /// (roster/snapshot `use_held`) and available to engine consumers.
    #[inline]
    pub fn using(&self) -> bool {
        self.input.intent_use_held && self.input.intent_gameplay
    }

    /// This session's open GUI, as the mod roster publishes it — read from
    /// the ONE place a session's open GUI lives, so the `GuiViewers` answer
    /// can never drift out of step with the panel.
    pub fn open_gui(&self) -> Option<crate::events::OpenGui> {
        match self.sim.menu.target() {
            crate::menu::ContainerTarget::None => None,
            crate::menu::ContainerTarget::Gui { kind, anchor } => {
                Some(crate::events::OpenGui { kind, anchor })
            }
        }
    }

    /// A snapshot of the raw held-rotation state for the placement inputs —
    /// each family derives its own reading from it.
    #[inline]
    pub fn held_rotation_snapshot(&self) -> HeldRotation {
        self.input.held_rotation.clone()
    }

    #[inline]
    pub fn held_slab_rotation(&self) -> petramond_world::slab::SlabRotation {
        // The acting hand's item: the rotation is armed per item, so an
        // off-hand pass only inherits it when both hands hold the same item.
        self.input
            .held_rotation
            .slab_rotation(self.player.held().map(|st| st.item))
    }

    /// Step the player's active status effects one game tick and apply the
    /// consequences of every interval boundary that fired:
    /// [`Player::tick_effects`] owns the durations and reports the
    /// boundaries, the session owns what they do. Spectators keep ticking
    /// their durations too — an effect is wall-clock-like state, not a
    /// survival consequence — but healing a full or dead player is already a
    /// no-op inside [`Player::heal`]. Touches this session only.
    pub fn tick_effects(&mut self) {
        for behavior in self.player.tick_effects() {
            match behavior {
                petramond_world::effect::EffectBehavior::None
                | petramond_world::effect::EffectBehavior::Speed { .. } => {}
                petramond_world::effect::EffectBehavior::Regen { amount, .. } => {
                    self.player.heal(amount);
                }
            }
        }
    }

    /// The in-progress eat as `(progress / eat_ticks)` in `[0, 1)`, or `None`.
    pub fn eating_progress(&self) -> Option<f32> {
        let eat = self.sim.eating?;
        let ticks = eat.item.food()?.eat_ticks.max(1);
        Some(eat.progress as f32 / ticks as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tracker's water branch: a FALL into water reports a splash with the
    /// airborne drop; walking in at ground level (or swimming around once wet)
    /// reports nothing a threshold would keep.
    #[test]
    fn fall_tracker_reports_water_entries_only_for_real_falls() {
        // Fall 4 blocks off a ledge into water.
        let mut fall = FallTracker::new(68.0);
        assert_eq!(fall.observe(68.0, false, false), None, "left the ledge");
        assert_eq!(fall.observe(66.0, false, false), None, "mid-air");
        assert_eq!(
            fall.observe(64.0, false, true),
            Some(FallOutcome::Splashed(4.0)),
            "the water entry reports the whole drop"
        );
        // Swimming afterwards: per-observation drops are tiny bobs, never the
        // fall again.
        match fall.observe(63.8, false, true) {
            None => {}
            Some(FallOutcome::Splashed(d)) => {
                assert!(d < 0.5, "swimming reports only bob-sized drops: {d}")
            }
            other => panic!("unexpected {other:?}"),
        }

        // Walking into water at ground level: grounded observations, never a
        // splash.
        let mut walk = FallTracker::new(64.0);
        assert_eq!(walk.observe(64.0, true, false), None);
        assert_eq!(
            walk.observe(64.0, true, true),
            None,
            "a grounded (walked-in) water entry reports nothing"
        );

        // A dry landing still measures fall damage exactly as before.
        let mut dry = FallTracker::new(70.0);
        assert_eq!(dry.observe(70.0, false, false), None);
        assert_eq!(
            dry.observe(64.0, true, false),
            Some(FallOutcome::Landed(6.0)),
            "dry landings keep their distance"
        );
    }
}
