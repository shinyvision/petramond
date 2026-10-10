use crate::net::protocol::{ClientRequestId, TargetRef};
use crate::player::{Player, PlayerId};
use petramond_math::math::IVec3;
use petramond_world::gui_state::{MenuSlot, PointerButton};
use petramond_world::item::ItemType;

use super::HeldRotation;

pub const MENU_QUEUE_DEPTH: usize = 64;

pub const MENU_ACTIONS_PER_TICK: usize = 16;

pub const BREAK_QUEUE_DEPTH: usize = 16;

pub const CHAT_BURST: f64 = 6.0;
pub const CHAT_PER_SECOND: f64 = 1.0;

/// Client mod events one session may have accepted at once: a mod UI sends one per frame
/// while a control is dragged, and a network hitch delivers a second of them in one pump.
pub const MOD_EVENT_BURST: f64 = 128.0;
/// Sustained client mod events per second per session: one per frame at 60 fps.
pub const MOD_EVENT_PER_SECOND: f64 = 64.0;

#[derive(Copy, Clone, Debug)]
pub struct PendingBreakFinished {
    pub request_id: ClientRequestId,
    pub pos: IVec3,
    pub tool_item_id: Option<u16>,
    pub predicted: bool,
}

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

    pub fn ray_item(self) -> Option<ItemType> {
        let fluid_ray = |item: Option<ItemType>| item.filter(|i| i.use_ray().sees_fluid());
        fluid_ray(self.held_item)
            .or_else(|| fluid_ray(self.off_hand_item))
            .or(self.held_item)
    }

    #[inline]
    pub fn selection_still_matches(self, player: &Player) -> bool {
        player.inventory.active_slot() == self.held_slot
            && player.inventory.selected().map(|stack| stack.item) == self.held_item
            && player.inventory.off_hand().map(|stack| stack.item) == self.off_hand_item
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct AttackClick {
    pub mob: Option<u64>,
    pub player: Option<PlayerId>,
}

#[derive(Clone, Debug)]
pub enum PendingMenuAction {
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
    SwapOffHand {
        slot: MenuSlot,
        request_id: ClientRequestId,
    },
}

impl PendingMenuAction {
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

pub struct InputLatches {
    pub look: Option<TargetRef>,
    pub intent_break_held: bool,
    pub intent_use_held: bool,
    pub intent_sneak: bool,
    pub intent_gameplay: bool,
    pub use_repeat_cooldown: u32,
    attack: Option<AttackClick>,
    pub pending_use_click: Option<PendingUseClick>,
    pub held_rotation: HeldRotation,
    menu_actions: std::collections::VecDeque<PendingMenuAction>,
    break_finished: Vec<PendingBreakFinished>,
    pub deferred_break_finished: Option<PendingBreakFinished>,
    pub pending_break_ack: rustc_hash::FxHashMap<IVec3, u64>,
    pub move_wishdir: petramond_math::math::Vec3,
    pub move_jump: bool,
    pub move_sprint: bool,
    pub prev_sneak: bool,
    pub claim_pos: petramond_math::world_pos::WorldPos,
    pub claim_vel: petramond_math::math::Vec3,
    pub claim_on_ground: bool,
    pub claim_fresh: bool,
    pub ticks_since_claim: u32,
    pub wake_requested: bool,
    pub respawn_requested: bool,
    chat: crate::net::rate::TokenBucket,
    mod_events: crate::net::rate::TokenBucket,
}

impl InputLatches {
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
            mod_events: crate::net::rate::TokenBucket::new(
                MOD_EVENT_BURST,
                MOD_EVENT_PER_SECOND,
                std::time::Instant::now(),
            ),
        }
    }

    pub fn allow_chat(&mut self, now: std::time::Instant) -> bool {
        self.chat.try_take(1.0, now)
    }

    pub fn allow_mod_event(&mut self, now: std::time::Instant) -> bool {
        self.mod_events.try_take(1.0, now)
    }

    pub fn latch_attack(&mut self, click: AttackClick) {
        self.attack = Some(click);
    }

    pub fn attack(&self) -> Option<AttackClick> {
        self.attack
    }

    pub fn take_attack(&mut self) -> Option<AttackClick> {
        self.attack.take()
    }

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

    pub fn take_menu_actions(&mut self) -> Vec<PendingMenuAction> {
        let n = self.menu_actions.len().min(MENU_ACTIONS_PER_TICK);
        self.menu_actions.drain(..n).collect()
    }

    pub fn queued_menu_actions(&self) -> usize {
        self.menu_actions.len()
    }

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

    pub fn take_break_finished(&mut self) -> Vec<PendingBreakFinished> {
        std::mem::take(&mut self.break_finished)
    }

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

    #[test]
    fn chat_is_rate_limited_after_its_burst() {
        let mut latches = InputLatches::new(WorldPos::new(0.0, 64.0, 0.0));
        let now = std::time::Instant::now();
        let sent = (0..20).filter(|_| latches.allow_chat(now)).count();
        assert_eq!(sent, CHAT_BURST as usize);
        let later = now + std::time::Duration::from_secs_f64(1.5 / CHAT_PER_SECOND);
        assert!(latches.allow_chat(later), "the budget refills");
    }

    #[test]
    fn mod_events_are_rate_limited_after_their_burst() {
        let mut latches = InputLatches::new(WorldPos::new(0.0, 64.0, 0.0));
        let now = std::time::Instant::now();
        let sent = (0..1000).filter(|_| latches.allow_mod_event(now)).count();
        assert_eq!(sent, MOD_EVENT_BURST as usize);
        let later = now + std::time::Duration::from_secs_f64(1.5 / MOD_EVENT_PER_SECOND);
        assert!(latches.allow_mod_event(later), "the budget refills");
    }

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
