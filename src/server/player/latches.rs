//! A session's latched client input: the held intents and claimed transform
//! from the latest `PlayerUpdate`, and the one-shot edges and ordered
//! request queues its messages left for the fixed tick.
//!
//! The queues are bounded and private. A message adds to one only through
//! its `queue_*` method, which hands the request back when the queue is full
//! (the caller denies it at once — every request id still gets its
//! outcome); the tick takes them only through the `take_*` methods, and the
//! menu queue drains at most [`MENU_ACTIONS_PER_TICK`] per tick, so a
//! flooding client can neither grow server memory nor buy more than its
//! share of a tick.

use crate::net::protocol::{ClientRequestId, TargetRef};
use crate::player::{Player, PlayerId};
use petramond_math::math::IVec3;
use petramond_world::gui_state::{MenuSlot, PointerButton};
use petramond_world::item::ItemType;

use super::HeldRotation;

/// Most menu intents one session may have waiting at once. A person clicks
/// a handful per tick at most; beyond this the client is flooding.
pub const MENU_QUEUE_DEPTH: usize = 64;

/// Most menu intents one session applies per tick; the rest wait, in
/// arrival order, for the next tick.
pub const MENU_ACTIONS_PER_TICK: usize = 16;

/// Most `BreakFinished` requests one session may have waiting at once.
/// Instabreak can legitimately finish a few cells per tick window; never
/// dozens.
pub const BREAK_QUEUE_DEPTH: usize = 16;

/// Chat lines (and slash commands) one session may send in a burst, and
/// the sustained rate after it. Every line is broadcast to every session,
/// so this is what one client's spam can cost everybody.
pub const CHAT_BURST: f64 = 6.0;
pub const CHAT_PER_SECOND: f64 = 1.0;

/// One latched `BreakFinished` request, resolved by the mining stage against
/// the server's own observed mining window. Only the fields the resolution
/// needs — never the whole `PlayerAction` (the latch site would otherwise have
/// to re-prove the variant).
#[derive(Copy, Clone, Debug)]
pub struct PendingBreakFinished {
    pub request_id: ClientRequestId,
    pub pos: IVec3,
    /// Wire item id of the tool the client claims it used (`None` = bare hand).
    pub tool_item_id: Option<u16>,
    /// Whether the client presented the break optimistically — gates the
    /// initiator echo strip on accept (see `finish_player_break`).
    pub predicted: bool,
}

/// One buffered secondary-button press. The click-time selection is part of
/// the intent: receipt-time targeting and tick-time mutation must describe the
/// same held slot/item, even when a newer `PlayerUpdate` changes the hotbar
/// before the Placement stage consumes the click.
#[derive(Copy, Clone, Debug)]
pub struct PendingUseClick {
    pub mob: Option<u64>,
    pub target: Option<TargetRef>,
    pub request_id: Option<ClientRequestId>,
    pub predicted: bool,
    pub jabbed: bool,
    held_slot: u8,
    held_item: Option<ItemType>,
    off_hand_item: Option<ItemType>,
}

impl PendingUseClick {
    pub fn capture(
        player: &Player,
        mob: Option<u64>,
        target: Option<TargetRef>,
        request_id: Option<ClientRequestId>,
        predicted: bool,
        jabbed: bool,
    ) -> Self {
        Self {
            mob,
            target,
            request_id,
            predicted,
            jabbed,
            held_slot: player.inventory.active_slot(),
            held_item: player.inventory.selected().map(|stack| stack.item),
            off_hand_item: player.inventory.off_hand().map(|stack| stack.item),
        }
    }

    #[inline]
    pub fn held_item(self) -> Option<ItemType> {
        self.held_item
    }

    #[inline]
    pub fn off_hand_item(self) -> Option<ItemType> {
        self.off_hand_item
    }

    /// The item that selects the click's RAY: the first hand (main, then off)
    /// holding an item whose `use_ray` sees fluid, else the main-hand item. The
    /// off-hand boat needs the water target for the ladder's second pass.
    pub fn ray_item(self) -> Option<ItemType> {
        let fluid_ray = |item: Option<ItemType>| item.filter(|i| i.use_ray().sees_fluid());
        fluid_ray(self.held_item)
            .or_else(|| fluid_ray(self.off_hand_item))
            .or(self.held_item)
    }

    /// Both hands must still hold what the click captured: which hand acts is
    /// decided during dispatch (main pass first, then off), so a change to
    /// EITHER hand between receipt and the Placement stage denies the whole
    /// attempt instead of letting the ladder act on an item the click never
    /// aimed.
    #[inline]
    pub fn selection_still_matches(self, player: &Player) -> bool {
        player.inventory.active_slot() == self.held_slot
            && player.inventory.selected().map(|stack| stack.item) == self.held_item
            && player.inventory.off_hand().map(|stack| stack.item) == self.off_hand_item
    }
}

/// One buffered primary-button press and the targets the client resolved
/// for it at click time — at most one of a mob (by STABLE id: despawns
/// shift indices between the click and the tick) or a player (PvP). Both
/// are claims, validated when the Attack stage consumes the press.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct AttackClick {
    pub mob: Option<u64>,
    pub player: Option<PlayerId>,
}

/// One ordered menu intent. Open, close, slot clicks, and explicit crafts
/// share a queue so network arrival order is also simulation order.
#[derive(Clone, Debug)]
pub enum PendingMenuAction {
    /// Open the GUI session for `kind` — engine containers and mod GUIs ride
    /// this one lane. `anchor` is the block or mob the session opens on (`None`
    /// for the inventory key and unanchored `GuiOpen`s); per-kind session semantics
    /// resolve at the menu stage's kind dispatch, not here.
    OpenGui {
        kind: petramond_world::gui_state::GuiKind,
        anchor: Option<crate::menu::MenuAnchor>,
    },
    Close,
    CreativeCursor {
        item: Option<String>,
        request_id: ClientRequestId,
    },
    SlotClick {
        slot: MenuSlot,
        button: PointerButton,
        shift: bool,
        gather: bool,
        request_id: ClientRequestId,
    },
    SlotDrag {
        slots: Vec<MenuSlot>,
        button: PointerButton,
        request_id: ClientRequestId,
    },
    DropSlot {
        slot: MenuSlot,
        all: bool,
        request_id: ClientRequestId,
    },
    CraftRecipe {
        recipe: String,
        bulk: bool,
        request_id: ClientRequestId,
    },
    /// Swap the off-hand with a concrete slot (the F gesture — the selected
    /// hotbar slot in gameplay, the hovered slot in a menu). Rides this queue
    /// so it serializes with clicks against the same slots.
    SwapOffHand {
        slot: MenuSlot,
        request_id: ClientRequestId,
    },
}

impl PendingMenuAction {
    /// The client request this intent owes an outcome to (`None` for opens
    /// and closes, which answer through the menu sync instead).
    pub fn request_id(&self) -> Option<ClientRequestId> {
        match self {
            Self::OpenGui { .. } | Self::Close => None,
            Self::CreativeCursor { request_id, .. }
            | Self::SlotClick { request_id, .. }
            | Self::SlotDrag { request_id, .. }
            | Self::DropSlot { request_id, .. }
            | Self::CraftRecipe { request_id, .. }
            | Self::SwapOffHand { request_id, .. } => Some(*request_id),
        }
    }
}

/// Input intent latched from the most recent messages, consumed on the fixed
/// tick.
pub struct InputLatches {
    /// Block under this player's crosshair (block + face normal), latched from
    /// the most recent `PlayerUpdate` and reach-validated at the latch. `None`
    /// when a mob is the closer target.
    pub look: Option<TargetRef>,
    pub intent_break_held: bool,
    pub intent_use_held: bool,
    pub intent_sneak: bool,
    pub intent_gameplay: bool,
    /// Ticks until a HELD use button re-runs the use-click ladder — paces
    /// the hold-to-interact repeat (see `tick_use_repeat`).
    pub use_repeat_cooldown: u32,
    /// The buffered attack press (see [`latch_attack`](Self::latch_attack)).
    attack: Option<AttackClick>,
    /// The complete buffered use click, including its click-time held
    /// slot/item. Keeping the fields together makes supersede, menu-drop, and
    /// tick consumption atomic: no target/request/selection fragment can
    /// survive after the click itself is gone.
    pub pending_use_click: Option<PendingUseClick>,
    /// The held block's placement rotation, fed from `PlayerUpdate`'s raw
    /// counter (see [`HeldRotation::apply_wire`]). The placement paths read
    /// THIS copy, never the client's.
    pub held_rotation: HeldRotation,
    /// Menu transitions and mutations latched since the last tick, applied in
    /// one arrival-ordered stream (bounded: [`MENU_QUEUE_DEPTH`]).
    menu_actions: std::collections::VecDeque<PendingMenuAction>,
    /// Latched `BreakFinished` requests, applied by the mining stage in
    /// arrival order. A queue, not a single slot: instabreak blocks can
    /// legitimately finish two cells in one tick window, so each finish must
    /// resolve independently rather than supersede the last (bounded:
    /// [`BREAK_QUEUE_DEPTH`]).
    break_finished: Vec<PendingBreakFinished>,
    /// A `BreakFinished` that arrived before the server's observed mining
    /// window was full (`TooFast`). Kept until the hold-path timer finishes
    /// the same cell (then accepted + presentation stripped) or mining
    /// abandons the cell (then denied + corrective). Avoids deny→restore→
    /// hold-path double presentation on slow links.
    pub deferred_break_finished: Option<PendingBreakFinished>,
    /// Cells this session already broke (hold-path or BreakFinished) that
    /// still owe a `BreakFinished` accept, with the world tick each was
    /// broken on. A lagged finish for an already-air cell in this set is
    /// accepted (no restore); air without an entry is a real deny. Cleared
    /// when the matching finish is answered, or expired after
    /// `BREAK_ACK_TTL_TICKS` (a hold-path break whose finish never arrives
    /// must not grow the set forever).
    pub pending_break_ack: rustc_hash::FxHashMap<IVec3, u64>,
    /// Movement intent from the latest `PlayerUpdate` (F2 server integrate).
    pub move_wishdir: petramond_math::math::Vec3,
    pub move_jump: bool,
    pub move_sprint: bool,
    /// Last tick's sneak level — the rising edge while mounted is the
    /// dismount gesture (there is deliberately no other server-side sneak
    /// edge state; see `ConnectedPlayer::sneaking`).
    pub prev_sneak: bool,
    /// Client-predicted transform from the latest `PlayerUpdate` (F1 soft accept).
    pub claim_pos: petramond_math::world_pos::WorldPos,
    pub claim_vel: petramond_math::math::Vec3,
    pub claim_on_ground: bool,
    /// Set by `PlayerUpdate`; cleared after `tick_movement` consumes the claim.
    /// Stale claims must not yank the player back every tick.
    pub claim_fresh: bool,
    /// Ticks integrated since the last consumed claim — how stale the
    /// client's report is. A slow client legitimately drifts further from the
    /// server's free-running integration, so both the F1 closeness ring and
    /// the `SelfTransform` correction deadband scale with this.
    pub ticks_since_claim: u32,
    pub wake_requested: bool,
    pub respawn_requested: bool,
    /// The chat budget (see [`allow_chat`](Self::allow_chat)).
    chat: crate::net::rate::TokenBucket,
}

impl InputLatches {
    /// Nothing latched yet, with the claimed transform anchored at `pos`.
    pub fn new(pos: petramond_math::world_pos::WorldPos) -> Self {
        Self {
            look: None,
            intent_break_held: false,
            intent_use_held: false,
            intent_sneak: false,
            intent_gameplay: false,
            use_repeat_cooldown: 0,
            attack: None,
            pending_use_click: None,
            held_rotation: HeldRotation::default(),
            menu_actions: std::collections::VecDeque::new(),
            break_finished: Vec::new(),
            deferred_break_finished: None,
            pending_break_ack: Default::default(),
            move_wishdir: petramond_math::math::Vec3::ZERO,
            move_jump: false,
            move_sprint: false,
            prev_sneak: false,
            claim_pos: pos,
            claim_vel: petramond_math::math::Vec3::ZERO,
            claim_on_ground: false,
            claim_fresh: false,
            ticks_since_claim: 0,
            wake_requested: false,
            respawn_requested: false,
            chat: crate::net::rate::TokenBucket::new(
                CHAT_BURST,
                CHAT_PER_SECOND,
                std::time::Instant::now(),
            ),
        }
    }

    /// Spend one line of the session's chat budget at `now`; `false` when
    /// the session is sending faster than [`CHAT_PER_SECOND`] after its
    /// [`CHAT_BURST`].
    pub fn allow_chat(&mut self, now: std::time::Instant) -> bool {
        self.chat.try_take(1.0, now)
    }

    /// Buffer an attack press with its click-time targets. ONE deep: a newer
    /// press replaces the targets the buffered one will be validated against.
    pub fn latch_attack(&mut self, click: AttackClick) {
        self.attack = Some(click);
    }

    /// The buffered attack press, if any (read-only).
    pub fn attack(&self) -> Option<AttackClick> {
        self.attack
    }

    /// Spend the buffered attack press.
    pub fn take_attack(&mut self) -> Option<AttackClick> {
        self.attack.take()
    }

    /// Queue one menu intent behind the ones already waiting. A full queue
    /// hands the intent back, for the caller to deny.
    pub fn queue_menu_action(
        &mut self,
        action: PendingMenuAction,
    ) -> Result<(), PendingMenuAction> {
        if self.menu_actions.len() >= MENU_QUEUE_DEPTH {
            return Err(action);
        }
        self.menu_actions.push_back(action);
        Ok(())
    }

    /// This tick's share of the queued menu intents, oldest first: at most
    /// [`MENU_ACTIONS_PER_TICK`]; the rest keep their order for the next tick.
    pub fn take_menu_actions(&mut self) -> Vec<PendingMenuAction> {
        let n = self.menu_actions.len().min(MENU_ACTIONS_PER_TICK);
        self.menu_actions.drain(..n).collect()
    }

    /// How many menu intents are waiting.
    pub fn queued_menu_actions(&self) -> usize {
        self.menu_actions.len()
    }

    /// Queue one `BreakFinished` behind the ones already waiting. A full
    /// queue hands the request back, for the caller to deny.
    pub fn queue_break_finished(
        &mut self,
        request: PendingBreakFinished,
    ) -> Result<(), PendingBreakFinished> {
        if self.break_finished.len() >= BREAK_QUEUE_DEPTH {
            return Err(request);
        }
        self.break_finished.push(request);
        Ok(())
    }

    /// Every queued `BreakFinished`, oldest first.
    pub fn take_break_finished(&mut self) -> Vec<PendingBreakFinished> {
        std::mem::take(&mut self.break_finished)
    }

    /// Drop the queued action edges when a screen takes input focus, so
    /// clicks cannot fire behind it. Returns the buffered use click's
    /// request id, which still owes the client a (denying) outcome.
    pub fn drop_action_edges(&mut self) -> Option<ClientRequestId> {
        self.intent_break_held = false;
        self.intent_use_held = false;
        self.attack = None;
        self.pending_use_click
            .take()
            .and_then(|click| click.request_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use petramond_math::world_pos::WorldPos;

    fn click(request_id: ClientRequestId) -> PendingMenuAction {
        PendingMenuAction::DropSlot {
            slot: MenuSlot::Inventory(0),
            all: false,
            request_id,
        }
    }

    /// The menu queue refuses past its depth (handing the intent back with
    /// its request id) and drains a bounded, ordered share per tick.
    #[test]
    fn the_menu_queue_is_bounded_and_drains_a_share_per_tick() {
        let mut latches = InputLatches::new(WorldPos::new(0.0, 64.0, 0.0));
        for id in 0..MENU_QUEUE_DEPTH as u32 {
            assert!(latches.queue_menu_action(click(id)).is_ok());
        }
        let refused = latches
            .queue_menu_action(click(999))
            .expect_err("a full queue refuses");
        assert_eq!(refused.request_id(), Some(999));

        let first = latches.take_menu_actions();
        assert_eq!(first.len(), MENU_ACTIONS_PER_TICK);
        assert_eq!(first[0].request_id(), Some(0), "oldest first");
        assert_eq!(
            latches.queued_menu_actions(),
            MENU_QUEUE_DEPTH - MENU_ACTIONS_PER_TICK,
            "the rest wait for the next tick"
        );
        assert_eq!(
            latches.take_menu_actions()[0].request_id(),
            Some(MENU_ACTIONS_PER_TICK as u32),
            "and keep their order"
        );
    }

    #[test]
    fn the_break_queue_is_bounded() {
        let mut latches = InputLatches::new(WorldPos::new(0.0, 64.0, 0.0));
        let finish = |request_id| PendingBreakFinished {
            request_id,
            pos: IVec3::ZERO,
            tool_item_id: None,
            predicted: false,
        };
        for id in 0..BREAK_QUEUE_DEPTH as u32 {
            assert!(latches.queue_break_finished(finish(id)).is_ok());
        }
        assert_eq!(
            latches
                .queue_break_finished(finish(77))
                .expect_err("full")
                .request_id,
            77
        );
        assert_eq!(latches.take_break_finished().len(), BREAK_QUEUE_DEPTH);
        assert!(latches.take_break_finished().is_empty());
    }

    /// Chat allows its burst, then only the sustained rate.
    #[test]
    fn chat_is_rate_limited_after_its_burst() {
        let mut latches = InputLatches::new(WorldPos::new(0.0, 64.0, 0.0));
        let now = std::time::Instant::now();
        let sent = (0..20).filter(|_| latches.allow_chat(now)).count();
        assert_eq!(sent, CHAT_BURST as usize);
        let later = now + std::time::Duration::from_secs_f64(1.5 / CHAT_PER_SECOND);
        assert!(latches.allow_chat(later), "the budget refills");
    }

    /// Focus loss drops every action edge at once and surfaces the use
    /// click's request id so it can still be answered.
    #[test]
    fn dropping_action_edges_spares_no_fragment() {
        let mut latches = InputLatches::new(WorldPos::new(0.0, 64.0, 0.0));
        latches.intent_break_held = true;
        latches.latch_attack(AttackClick {
            mob: Some(3),
            player: None,
        });
        let player = Player::new(WorldPos::new(0.0, 64.0, 0.0));
        latches.pending_use_click = Some(PendingUseClick::capture(
            &player,
            None,
            None,
            Some(5),
            true,
            false,
        ));
        assert_eq!(latches.drop_action_edges(), Some(5));
        assert!(!latches.intent_break_held);
        assert_eq!(latches.attack(), None);
        assert!(latches.pending_use_click.is_none());
    }
}
