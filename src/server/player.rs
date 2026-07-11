//! Per-connected-player simulation state.
//!
//! Everything the sim tracks *per player* lives here — the fields that used to
//! sit directly on `Game` when the game was single-player. The tick stages
//! (`src/game/*.rs`) loop the sessions in id order; the client-facing session
//! is always index 0 until remote connections exist (multiplayer Phase C+).
//!
//! Since Phase C2b input reaches a session only through `net::protocol`
//! messages (`ServerGame::apply_message`): `PlayerUpdate` latches the
//! transform, targeting, and held intents; `PlayerAction`/`MenuClick` latch the
//! one-shot edges. The tick stages consume the latches exactly as before.

use crate::block::RenderShape;
use crate::block_state::{HeldBlockState, LogAxis, SlabState, StairHalf, StairState};
use crate::controls::PointerButton;
use crate::game::ContainerMenu;
use crate::gui::MenuSlot;
use crate::item::ItemType;
use crate::mathh::IVec3;
use crate::mining::MiningState;
use crate::net::protocol::TargetRef;
use crate::player::Player;
use crate::server::bed::SleepState;
use crate::server::drops::DropQueue;
use crate::server::item_use::EatingState;

/// Session-scoped player identity: index-stable for a connection's lifetime,
/// re-used after disconnect. Rides the wire as a single byte. (`pub` for the
/// `pub` mob-API fields that carry it; the `server` module is crate-private.)
#[derive(
    Copy,
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct PlayerId(pub u8);

/// The held block's placement-rotation state (the R-key cycle): which item the
/// cycle was armed on and the raw counter. Lives BOTH client-side (the client
/// owns the R key and previews the rotated held block) and session-side (the
/// placement paths read the session's copy, fed from `PlayerUpdate`'s raw
/// counter) — one struct so the two can never drift in logic.
#[derive(Clone, Debug, Default)]
pub(crate) struct HeldRotation {
    pub item: Option<ItemType>,
    pub rotation: u8,
}

impl HeldRotation {
    /// Cycle the rotation for `selected` (stairs upside-down, slab column/row,
    /// log axis). Selecting a non-rotatable item clears it.
    pub(crate) fn toggle(&mut self, selected: Option<ItemType>) {
        let Some(item) = selected else {
            self.clear();
            return;
        };
        if !item.as_block().is_some_and(rotatable_block) {
            self.clear();
            return;
        }
        if self.item == Some(item) {
            let count = item.as_block().map_or(1, rotation_count).max(1);
            self.rotation = (self.rotation + 1) % count;
        } else {
            self.item = Some(item);
            self.rotation = 1 % item.as_block().map_or(1, rotation_count).max(1);
        }
    }

    #[inline]
    pub(crate) fn clear(&mut self) {
        self.item = None;
        self.rotation = 0;
    }

    /// Latch the raw counter a `PlayerUpdate` carried. The wire carries ONLY
    /// the counter; the session re-derives the armed item as its own currently
    /// selected item whenever the counter changes to nonzero (the client
    /// resets the counter to 0 on every hotbar change, so a changed nonzero
    /// counter can only mean an R-press on the current selection). An
    /// unchanged counter keeps the armed item as-is, preserving the
    /// "rotation is remembered per item" activity check.
    pub(crate) fn apply_wire(&mut self, counter: u8, selected: Option<ItemType>) {
        if counter == self.rotation {
            return;
        }
        if counter == 0 {
            self.clear();
        } else {
            self.rotation = counter;
            self.item = selected;
        }
    }

    #[inline]
    fn active(&self, selected: Option<ItemType>) -> bool {
        let Some(item) = selected else {
            return false;
        };
        self.item == Some(item)
            && self.rotation != 0
            && item.as_block().is_some_and(rotatable_block)
    }

    #[inline]
    pub(crate) fn held_block_state(&self, selected: Option<ItemType>) -> HeldBlockState {
        let Some(block) = selected.and_then(ItemType::as_block) else {
            return HeldBlockState::None;
        };
        if block.render_shape() == RenderShape::Stair {
            return HeldBlockState::Stair(StairState::new(
                crate::block_model::DEFAULT_MODEL_FACING,
                self.stair_half(selected),
            ));
        }
        if block.render_shape() == RenderShape::Slab {
            let slot = crate::slab::slot_for_rotation(
                self.slab_rotation(selected),
                IVec3::ZERO,
                crate::facing::Facing::South,
            );
            return HeldBlockState::Slab(SlabState::single(slot.split, slot.index, block));
        }
        if block.is_log() {
            return HeldBlockState::Log(if self.active(selected) {
                LogAxis::X
            } else {
                LogAxis::Y
            });
        }
        HeldBlockState::None
    }

    #[inline]
    pub(crate) fn stair_half(&self, selected: Option<ItemType>) -> StairHalf {
        if self.active(selected) {
            StairHalf::Top
        } else {
            StairHalf::Bottom
        }
    }

    #[inline]
    pub(crate) fn slab_rotation(&self, selected: Option<ItemType>) -> crate::slab::SlabRotation {
        if self.active(selected) {
            crate::slab::SlabRotation::from_index(self.rotation)
        } else {
            crate::slab::SlabRotation::Bottom
        }
    }

    #[inline]
    pub(crate) fn log_axis_for_facing(
        &self,
        selected: Option<ItemType>,
        facing: crate::facing::Facing,
    ) -> LogAxis {
        if !self.active(selected) {
            return LogAxis::Y;
        }
        match facing {
            crate::facing::Facing::East | crate::facing::Facing::West => LogAxis::X,
            crate::facing::Facing::North | crate::facing::Facing::South => LogAxis::Z,
        }
    }
}

/// One latched `BreakFinished` request, resolved by the mining stage against
/// the server's own observed mining window. Only the fields the resolution
/// needs — never the whole `PlayerAction` (the latch site would otherwise have
/// to re-prove the variant).
#[derive(Copy, Clone, Debug)]
pub(crate) struct PendingBreakFinished {
    pub request_id: crate::net::protocol::ClientRequestId,
    pub pos: IVec3,
    /// Wire item id of the tool the client claims it used (`None` = bare hand).
    pub tool_item_id: Option<u8>,
    /// Whether the client presented the break optimistically — gates the
    /// initiator echo strip on accept (see `finish_player_break`).
    pub predicted: bool,
}

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
pub(crate) struct FallTracker {
    peak_y: f32,
    airborne: bool,
}

impl FallTracker {
    pub(crate) fn new(y: f32) -> Self {
        Self {
            peak_y: y,
            airborne: false,
        }
    }

    /// Re-anchor at `y` and drop any airborne state — teleports and mode
    /// switches are never falls (mirrors `Player::teleport`/`set_mode`).
    pub(crate) fn reset(&mut self, y: f32) {
        self.peak_y = y;
        self.airborne = false;
    }

    /// Feed one reported transform. Returns what the update concluded: a dry
    /// landing (airborne → grounded) with its fall distance, or a water entry
    /// (airborne → in water) with the distance fallen into the surface —
    /// walking into water arrives grounded/level and reports nothing.
    pub(crate) fn observe(
        &mut self,
        y: f32,
        on_ground: bool,
        in_water: bool,
    ) -> Option<FallOutcome> {
        if in_water {
            let was_airborne = self.airborne;
            let dist = self.peak_y - y;
            self.peak_y = y;
            self.airborne = !on_ground;
            return (was_airborne && dist > 0.0).then_some(FallOutcome::Splashed(dist));
        }
        if on_ground {
            let landed = self.airborne;
            let dist = self.peak_y - y;
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
pub(crate) enum FallOutcome {
    /// Airborne → grounded, dry: fall damage applies.
    Landed(f32),
    /// Airborne → in water: no damage (water breaks the fall), but a hard
    /// enough entry throws the water-splash burst.
    Splashed(f32),
}

/// One player's simulation session: authoritative player state plus every
/// per-player latch, timer, and menu session the tick stages consume.
pub(crate) struct ConnectedPlayer {
    pub id: PlayerId,
    /// Display / save-key name (`players/<name>.dat`). The local session takes
    /// the client's configured name (`save::client::resolve_player_name`).
    pub name: String,
    pub player: Player,
    /// Block under this player's crosshair (block + face normal), latched from
    /// the most recent `PlayerUpdate` and reach-validated at the latch. `None`
    /// when a mob is the closer target.
    pub look: Option<TargetRef>,
    pub mining: MiningState,
    pub attack_cooldown: u32,
    // --- Input intent latched from the most recent message, consumed on the
    // fixed tick. ---
    pub intent_break_held: bool,
    pub intent_use_held: bool,
    pub intent_sneak: bool,
    pub intent_gameplay: bool,
    pub pending_attack: bool,
    /// The STABLE id of the mob the attack click targeted, resolved client-side
    /// at click time and re-resolved to an index at consume time (despawns
    /// shift indices between the click and the tick).
    pub pending_attack_mob: Option<u64>,
    /// The `PlayerId` byte of the PLAYER the attack click targeted (PvP; at
    /// most one of mob/player rides a click). Validated at consume time —
    /// exists, alive, non-spectator, within reach.
    pub pending_attack_player: Option<u8>,
    pub pending_place: bool,
    /// The stable id of the mob under the crosshair when the use click fired —
    /// the shear target.
    pub pending_use_mob: Option<u64>,
    /// The block target the use click carried (crosshair AT CLICK TIME,
    /// reach-validated at the latch). The interact/place ladder resolves
    /// against THIS cell, never the fresher look latch — a click racing the
    /// crosshair must land where the client's ghost is.
    pub pending_place_target: Option<TargetRef>,
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
    /// The held block's placement rotation, fed from `PlayerUpdate`'s raw
    /// counter (see [`HeldRotation::apply_wire`]). The placement paths read
    /// THIS copy, never the client's.
    pub held_rotation: HeldRotation,
    /// Server-side fall measurement from the reported transforms.
    pub fall: FallTracker,
    /// Hardest landing (blocks) since the tick last consumed it, measured by
    /// [`fall`](Self::fall) — `tick_fall_damage` converts it into damage.
    pub pending_fall: f32,
    /// Hardest fall INTO WATER (blocks) since the tick last consumed it —
    /// `tick_water_splash` converts it into the `petramond:water_splash` burst.
    pub pending_splash: f32,
    /// Player position when this frame's fixed ticks began — a tick-side
    /// position change is a teleport, which re-anchors [`fall`](Self::fall)
    /// (see `ServerGame::pump`).
    pub pos_before_ticks: crate::mathh::Vec3,
    /// Whether the LAST tick window teleported this player (the drift check
    /// over [`pos_before_ticks`](Self::pos_before_ticks)) — replicated as
    /// `PlayerStateRow::snap` so observers skip interpolating across the
    /// jump. Replication bookkeeping, refreshed every pump.
    pub tick_teleported: bool,
    /// The in-progress eat (held secondary button on food), or `None`.
    pub eating: Option<EatingState>,
    pub drop_queue: DropQueue,
    /// Container-menu clicks latched since the last tick, applied in order.
    /// Each carries the client's [`ClientRequestId`] for [`ActionOutcome`].
    pub pending_menu_clicks: Vec<(
        MenuSlot,
        PointerButton,
        bool,
        bool,
        crate::net::protocol::ClientRequestId,
    )>,
    /// Outcomes queued this tick window for the next `TickUpdate`.
    pub pending_action_outcomes: Vec<crate::net::protocol::ActionOutcome>,
    /// Latched `BreakFinished` request, applied by the mining stage.
    pub pending_break_finished: Option<PendingBreakFinished>,
    /// A `BreakFinished` that arrived before the server's observed mining
    /// window was full (`TooFast`). Kept until the hold-path timer finishes
    /// the same cell (then accepted + presentation stripped) or mining
    /// abandons the cell (then denied + corrective). Avoids deny→restore→
    /// hold-path double presentation on slow links — see
    /// WIKI/client-prediction.md.
    pub deferred_break_finished: Option<PendingBreakFinished>,
    /// Cells this session already broke (hold-path or BreakFinished) that
    /// still owe a `BreakFinished` accept, with the world tick each was
    /// broken on. A lagged finish for an already-air cell in this set is
    /// accepted (no restore); air without an entry is a real deny. Cleared
    /// when the matching finish is answered, or expired after
    /// `BREAK_ACK_TTL_TICKS` (a hold-path break whose finish never arrives
    /// must not grow the set forever).
    pub pending_break_ack: rustc_hash::FxHashMap<IVec3, u64>,
    /// Optional request id on the pending use/place click (place ghost ack).
    pub pending_place_request_id: Option<crate::net::protocol::ClientRequestId>,
    /// Whether the latched place click PRESENTED client-side (full ghost) —
    /// gates the initiator's `BlockPlaced` echo strip, never validation.
    pub pending_place_predicted: bool,
    /// Movement intent from the latest `PlayerUpdate` (F2 server integrate).
    pub move_wishdir: crate::mathh::Vec3,
    pub move_jump: bool,
    pub move_sprint: bool,
    /// Client-predicted transform from the latest `PlayerUpdate` (F1 soft accept).
    pub claim_pos: crate::mathh::Vec3,
    pub claim_vel: crate::mathh::Vec3,
    pub claim_on_ground: bool,
    /// Set by `PlayerUpdate`; cleared after `tick_movement` consumes the claim.
    /// Stale claims must not yank the player back every tick.
    pub claim_fresh: bool,
    /// Ticks integrated since the last consumed claim — how stale the
    /// client's report is. A slow client legitimately drifts further from the
    /// server's free-running integration, so both the F1 closeness ring and
    /// the `SelfTransform` correction deadband scale with this.
    pub ticks_since_claim: u32,
    /// The open container GUI's persistent edit target for THIS player.
    pub menu: ContainerMenu,
    /// The in-flight sleep session (`None` = awake).
    pub sleep: Option<SleepState>,
    pub wake_requested: bool,
    pub respawn_requested: bool,
    /// `PlayerAction::OpenInventory` latched; the Menu stage opens the 2×2
    /// crafting session on the tick.
    pub open_inventory_requested: bool,
    /// `PlayerAction::CloseMenu` latched; the tick closes the open menu
    /// session at its start (cursor stash, craft-grid return, viewer release).
    pub close_menu_requested: bool,
    // --- One-shot outbox: screen/effect requests the tick queues for this
    // player's client, consumed into its `SelfEvents` per replication batch
    // (INTERNAL since C2c-iii — the client only sees `OpenScreen`). ---
    pub request_open_inventory: bool,
    pub request_open_table: bool,
    pub request_open_furnace: Option<IVec3>,
    pub request_open_chest: Option<IVec3>,
    pub request_open_workbench: bool,
    pub request_open_mod_gui: Option<(crate::gui::GuiKind, Option<IVec3>)>,
    pub request_close_mod_gui: bool,
    pub request_open_sleep: bool,
    /// The open mod-GUI session's state map (written by mods on the tick via
    /// `GuiStateSet`, cleared by the menu funnels on open/close). Snapshotted
    /// behind the `Arc` per replication batch — copy-on-write on writes.
    pub gui_state: std::sync::Arc<crate::gui::GuiStateMap>,
    /// The inventory revision the last emitted `SelfState` carried a full
    /// inventory for — per-recipient replication bookkeeping, not sim state.
    /// `None` = nothing sent yet, so the first update after join always
    /// includes the inventory.
    pub last_sent_inventory_revision: Option<u64>,
    /// The last `MenuSyncMsg` this session was sent (its `gui_state` field
    /// always `None` — the map compares by `Arc` identity below). On-change
    /// send detection; replication bookkeeping, not sim state.
    pub last_menu_sync: Option<crate::net::protocol::MenuSyncMsg>,
    /// The `gui_state` map allocation the last sync shipped. Holding the
    /// `Arc` is what makes identity comparison sound: the next tick-side
    /// write is forced to copy-on-write onto a fresh allocation.
    pub last_sent_gui_state: Option<std::sync::Arc<crate::gui::GuiStateMap>>,
    /// Per-connection terrain replication state (which columns/sections this
    /// client holds) — see `server::streaming`.
    pub terrain: crate::server::streaming::TerrainSync,
    /// The transform of the last `PlayerUpdate` this session applied — what
    /// the CLIENT last claimed. After the ticks, a session transform that no
    /// longer matches it means the tick moved the player (teleport,
    /// knockback): the next `SelfState` ships a [`SelfTransform`] correction.
    /// Replication bookkeeping, not sim state.
    ///
    /// [`SelfTransform`]: crate::net::protocol::SelfTransform
    pub last_reported_transform: Option<crate::net::protocol::SelfTransform>,
}

impl ConnectedPlayer {
    pub(crate) fn new(id: PlayerId, name: String, player: Player) -> Self {
        let fall = FallTracker::new(player.pos.y);
        let pos_before_ticks = player.pos;
        Self {
            id,
            name,
            player,
            look: None,
            mining: MiningState::new(),
            attack_cooldown: 0,
            intent_break_held: false,
            intent_use_held: false,
            intent_sneak: false,
            intent_gameplay: false,
            pending_attack: false,
            pending_attack_mob: None,
            pending_attack_player: None,
            pending_place: false,
            pending_use_mob: None,
            pending_place_target: None,
            pending_corrective_cells: Vec::new(),
            presented_places: Vec::new(),
            presented_breaks: Vec::new(),
            held_rotation: HeldRotation::default(),
            fall,
            pending_fall: 0.0,
            pending_splash: 0.0,
            pos_before_ticks,
            tick_teleported: false,
            eating: None,
            drop_queue: DropQueue::default(),
            pending_menu_clicks: Vec::new(),
            pending_action_outcomes: Vec::new(),
            pending_break_finished: None,
            deferred_break_finished: None,
            pending_break_ack: Default::default(),
            pending_place_request_id: None,
            pending_place_predicted: false,
            move_wishdir: crate::mathh::Vec3::ZERO,
            move_jump: false,
            move_sprint: false,
            claim_pos: pos_before_ticks,
            claim_vel: crate::mathh::Vec3::ZERO,
            claim_on_ground: false,
            claim_fresh: false,
            ticks_since_claim: 0,
            menu: ContainerMenu::new(),
            sleep: None,
            wake_requested: false,
            respawn_requested: false,
            open_inventory_requested: false,
            close_menu_requested: false,
            request_open_inventory: false,
            request_open_table: false,
            request_open_furnace: None,
            request_open_chest: None,
            request_open_workbench: false,
            request_open_mod_gui: None,
            request_close_mod_gui: false,
            request_open_sleep: false,
            gui_state: crate::gui::empty_gui_state(),
            last_sent_inventory_revision: None,
            last_menu_sync: None,
            last_sent_gui_state: None,
            terrain: Default::default(),
            last_reported_transform: None,
        }
    }

    #[inline]
    pub(crate) fn selected_item(&self) -> Option<ItemType> {
        self.player.inventory.selected().map(|s| s.item)
    }

    #[inline]
    pub(crate) fn held_stair_half(&self) -> StairHalf {
        self.held_rotation.stair_half(self.selected_item())
    }

    #[inline]
    pub(crate) fn held_slab_rotation(&self) -> crate::slab::SlabRotation {
        self.held_rotation.slab_rotation(self.selected_item())
    }

    #[inline]
    pub(crate) fn held_log_axis_for_facing(&self, facing: crate::facing::Facing) -> LogAxis {
        self.held_rotation
            .log_axis_for_facing(self.selected_item(), facing)
    }

    /// The in-progress eat as `(progress / eat_ticks)` in `[0, 1)`, or `None`.
    pub(crate) fn eating_progress(&self) -> Option<f32> {
        let eat = self.eating?;
        let ticks = eat.item.food()?.eat_ticks.max(1);
        Some(eat.progress as f32 / ticks as f32)
    }
}

fn rotatable_block(block: crate::block::Block) -> bool {
    matches!(block.render_shape(), RenderShape::Stair | RenderShape::Slab) || block.is_log()
}

fn rotation_count(block: crate::block::Block) -> u8 {
    if block.render_shape() == RenderShape::Slab {
        3
    } else {
        2
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
