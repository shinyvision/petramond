use crate::net::protocol::{
    ActionDenyReason, ActionOutcome, ClientRequestId, PlayerAction, TargetRef,
};
use crate::server::game::ServerGame;
use crate::server::player::{
    AttackClick, PendingBreakFinished, PendingMenuAction, PendingUseClick,
};
use petramond_math::math::IVec3;

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
                self.sessions[s].input.latch_attack(AttackClick {
                    mob,
                    player: player.map(crate::player::PlayerId),
                });
            }
            PlayerAction::Drop { all, request_id } => {
                let sess = &mut self.sessions[s];
                let slot = sess.player.inventory.active_slot();
                sess.sim
                    .drop_queue
                    .queue_selected(slot, all, Some(request_id));
            }
            PlayerAction::ThrowCursor { amount, request_id } => {
                let sess = &mut self.sessions[s];
                if !sess.sim.drop_queue.queue_cursor(
                    &sess.player.inventory,
                    amount,
                    Some(request_id),
                ) {
                    sess.replication
                        .pending_action_outcomes
                        .push(ActionOutcome::deny(request_id, ActionDenyReason::Denied));
                }
            }
            PlayerAction::BreakFinished {
                request_id,
                pos,
                tool_item_id,
                predicted,
            } => self.apply_break_finished(s, request_id, pos, tool_item_id, predicted),
            PlayerAction::ToggleMode => {
                if self.is_operator(s) {
                    let sess = &mut self.sessions[s];
                    sess.player.toggle_mode();
                    sess.sim.fall.reset(sess.player.pos.y);
                    sess.sim.pending_fall = 0.0;
                }
            }
            PlayerAction::ToggleCreative | PlayerAction::ToggleFlight => {
                if self.is_operator(s) && self.sessions[s].player.health() > 0 {
                    let sess = &mut self.sessions[s];
                    if action == PlayerAction::ToggleCreative {
                        sess.player.toggle_creative();
                    } else if sess.sim.mount.is_none() {
                        sess.player.toggle_creative_flight();
                    }
                    sess.sim.fall.reset(sess.player.pos.y);
                    sess.sim.pending_fall = 0.0;
                }
            }
            PlayerAction::Creative(action) => {
                if let crate::schematic::CreativeAction::Place {
                    digest,
                    origin,
                    turns,
                } = action
                {
                    self.request_placement(s, digest, origin, turns);
                    return;
                }
                self.sessions[s]
                    .sim
                    .creative
                    .try_enqueue(super::creative::Pending::Action(action));
            }
            PlayerAction::Schematic(request) => self.apply_schematic_request(s, request),
            PlayerAction::Wake => self.sessions[s].input.wake_requested = true,
            PlayerAction::Respawn => self.sessions[s].input.respawn_requested = true,
            PlayerAction::OpenInventory => {
                let kind = if self.sessions[s].player.abilities().item_catalog {
                    petramond_world::gui_state::GuiKind::Creative
                } else {
                    petramond_world::gui_state::GuiKind::Inventory
                };
                self.queue_menu_action(s, PendingMenuAction::OpenGui { kind, anchor: None })
            }
            PlayerAction::CloseMenu => self.queue_menu_action(s, PendingMenuAction::Close),
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
        click.target = self.authoritative_use_target(s, click.ray_item(), click.target);
        let sess = &mut self.sessions[s];
        if let Some(old) = sess
            .input
            .pending_use_click
            .replace(click)
            .and_then(|old| old.request_id)
        {
            sess.replication
                .pending_action_outcomes
                .push(ActionOutcome::deny(old, ActionDenyReason::Denied));
        }
    }

    fn apply_break_finished(
        &mut self,
        s: usize,
        request_id: ClientRequestId,
        pos: IVec3,
        tool_item_id: Option<u16>,
        predicted: bool,
    ) {
        let old_deferred = self.sessions[s].input.deferred_break_finished.take();
        if let Some(old) = old_deferred {
            self.sessions[s]
                .replication
                .pending_action_outcomes
                .push(ActionOutcome::deny(
                    old.request_id,
                    ActionDenyReason::Denied,
                ));
            let cells = self.world.break_footprint_cells(old.pos);
            self.sessions[s]
                .replication
                .pending_corrective_cells
                .extend(cells);
        }
        let request = PendingBreakFinished {
            request_id,
            pos,
            tool_item_id,
            predicted,
        };
        if let Err(refused) = self.sessions[s].input.queue_break_finished(request) {
            log::warn!(
                "session {} overflowed its break queue; denying",
                self.sessions[s].id.0
            );
            let cells = self.world.break_footprint_cells(refused.pos);
            let sess = &mut self.sessions[s];
            sess.replication.pending_corrective_cells.extend(cells);
            sess.replication.push_outcome(ActionOutcome::deny(
                refused.request_id,
                ActionDenyReason::Denied,
            ));
        }
    }

    pub(in crate::server) fn queue_menu_action(&mut self, s: usize, action: PendingMenuAction) {
        let sess = &mut self.sessions[s];
        let Err(refused) = sess.input.queue_menu_action(action) else {
            return;
        };
        log::warn!("session {} overflowed its menu queue; dropping", sess.id.0);
        if let Some(id) = refused.request_id() {
            sess.replication
                .push_outcome(ActionOutcome::deny(id, ActionDenyReason::Denied));
        }
    }

    pub fn push_action_outcome(
        &mut self,
        s: usize,
        id: ClientRequestId,
        accepted: bool,
        reason: Option<ActionDenyReason>,
    ) {
        self.sessions[s].replication.pending_action_outcomes.push(
            crate::net::protocol::ActionOutcome {
                id,
                accepted,
                reason,
            },
        );
    }
}
