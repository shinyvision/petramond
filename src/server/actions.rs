//! Message-time application of discrete [`PlayerAction`] intents: each click,
//! throw, or menu transition lands in its session's pending latch/queue here,
//! and the fixed tick consumes it in stage order.
//!
//! EVERY latched request id must eventually receive an `ActionOutcome` — an
//! unanswered id leaks the client's prediction-ledger entry forever. A
//! single-slot latch denies the id it supersedes; an intent that cannot even
//! queue is denied immediately.

use crate::game::prediction::deny;
use crate::mathh::IVec3;
use crate::net::protocol::{ActionDenyReason, ClientRequestId, PlayerAction, TargetRef};
use crate::server::game::ServerGame;
use crate::server::player::{PendingBreakFinished, PendingMenuAction, PendingUseClick};

impl ServerGame {
    pub(super) fn apply_action(&mut self, s: usize, action: PlayerAction) {
        match action {
            PlayerAction::UseClick {
                mob,
                target,
                request_id,
                predicted,
                jabbed,
            } => self.apply_use_click(s, mob, target, request_id, predicted, jabbed),
            PlayerAction::AttackClick { mob, player } => {
                let sess = &mut self.sessions[s];
                sess.pending_attack = true;
                sess.pending_attack_mob = mob;
                sess.pending_attack_player = player;
            }
            PlayerAction::Drop { all, request_id } => {
                let sess = &mut self.sessions[s];
                let slot = sess.player.inventory.active_slot();
                sess.drop_queue.queue_selected(slot, all, Some(request_id));
            }
            PlayerAction::ThrowCursorStack { request_id } => {
                let sess = &mut self.sessions[s];
                if !sess
                    .drop_queue
                    .queue_cursor_stack(&sess.player.inventory, Some(request_id))
                {
                    sess.pending_action_outcomes
                        .push(deny(request_id, ActionDenyReason::Denied));
                }
            }
            PlayerAction::ThrowCursorOne { request_id } => {
                let sess = &mut self.sessions[s];
                if !sess
                    .drop_queue
                    .queue_cursor_one(&sess.player.inventory, Some(request_id))
                {
                    sess.pending_action_outcomes
                        .push(deny(request_id, ActionDenyReason::Denied));
                }
            }
            PlayerAction::BreakFinished {
                request_id,
                pos,
                tool_item_id,
                predicted,
            } => self.apply_break_finished(s, request_id, pos, tool_item_id, predicted),
            // A mode switch is not tick input: applied at message time, like
            // the direct call it replaces. The floating spectator must never
            // be measured as falling — re-anchor the tracker (mirrors
            // `Player::set_mode`).
            PlayerAction::ToggleMode => {
                if self.is_operator(s) {
                    let sess = &mut self.sessions[s];
                    sess.player.toggle_mode();
                    sess.fall.reset(sess.player.pos.y);
                    sess.pending_fall = 0.0;
                }
            }
            PlayerAction::Wake => self.sessions[s].wake_requested = true,
            PlayerAction::Respawn => self.sessions[s].respawn_requested = true,
            // Menu transitions join clicks and crafts in one ordered queue so
            // arrival order remains authoritative on the fixed tick.
            PlayerAction::OpenInventory => self.sessions[s]
                .pending_menu_actions
                .push(PendingMenuAction::OpenInventory),
            PlayerAction::CloseMenu => self.sessions[s]
                .pending_menu_actions
                .push(PendingMenuAction::Close),
        }
    }

    fn apply_use_click(
        &mut self,
        s: usize,
        mob: Option<u64>,
        target: Option<TargetRef>,
        request_id: Option<ClientRequestId>,
        predicted: bool,
        jabbed: bool,
    ) {
        let mut click = PendingUseClick::capture(
            &self.sessions[s].player,
            mob,
            target,
            request_id,
            predicted,
            jabbed,
        );
        // A water-stopping ray is gameplay authority, not merely a client
        // presentation choice. The SAME captured item that selects this ray
        // must still occupy the captured slot when Placement consumes the
        // click.
        click.target = self.authoritative_use_target(s, click.held_item(), click.target);
        let sess = &mut self.sessions[s];
        if let Some(old) = sess
            .pending_use_click
            .replace(click)
            .and_then(|old| old.request_id)
        {
            sess.pending_action_outcomes
                .push(deny(old, ActionDenyReason::Denied));
        }
    }

    fn apply_break_finished(
        &mut self,
        s: usize,
        request_id: ClientRequestId,
        pos: IVec3,
        tool_item_id: Option<u8>,
        predicted: bool,
    ) {
        // A newer finish supersedes any in-flight latch OR deferred TooFast
        // wait — answer the old id so the ledger cannot leak.
        let (old_pending, old_deferred) = {
            let sess = &mut self.sessions[s];
            (
                sess.pending_break_finished.take(),
                sess.deferred_break_finished.take(),
            )
        };
        if let Some(old) = old_pending {
            self.sessions[s]
                .pending_action_outcomes
                .push(deny(old.request_id, ActionDenyReason::Denied));
        }
        if let Some(old) = old_deferred {
            self.sessions[s]
                .pending_action_outcomes
                .push(deny(old.request_id, ActionDenyReason::Denied));
            // Old optimistic clear may still be on the client.
            let cells = self.world.break_footprint_cells(old.pos);
            self.sessions[s].pending_corrective_cells.extend(cells);
        }
        self.sessions[s].pending_break_finished = Some(PendingBreakFinished {
            request_id,
            pos,
            tool_item_id,
            predicted,
        });
    }

    pub(crate) fn push_action_outcome(
        &mut self,
        s: usize,
        id: ClientRequestId,
        accepted: bool,
        reason: Option<ActionDenyReason>,
    ) {
        self.sessions[s]
            .pending_action_outcomes
            .push(crate::net::protocol::ActionOutcome {
                id,
                accepted,
                reason,
            });
    }
}
