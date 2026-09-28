use crate::net::protocol::{ClientToServer, PlayerUpdate, SelfTransform, ServerToClient};
use crate::player;
use crate::player::PlayerId;
use crate::server::player::PendingMenuAction;

use super::{PumpOutput, ServerGame};

impl ServerGame {
    #[cfg(any(test, feature = "test-support"))]
    pub fn pump(&mut self, dt: f32, inbox: &mut Vec<ClientToServer>) -> PumpOutput {
        let local = self.sessions[0].id;
        let mut tagged: Vec<(PlayerId, ClientToServer)> =
            inbox.drain(..).map(|msg| (local, msg)).collect();
        self.pump_tagged(dt, &mut tagged, &[])
    }

    pub fn pump_tagged(
        &mut self,
        dt: f32,
        inbox: &mut Vec<(PlayerId, ClientToServer)>,
        headroom: &[(PlayerId, usize)],
    ) -> PumpOutput {
        for (id, msg) in inbox.drain(..) {
            let Some(s) = self.sessions.index_of(id) else {
                continue;
            };
            if !self.sessions.is_faulted(s) {
                self.isolated(s, "message handling", |server| server.apply_message(s, msg));
            }
        }
        let mut kicked = self.evict_faulted();
        for sess in &mut self.sessions {
            sess.replication.pos_before_ticks = sess.player.pos;
        }
        let (mut events, ticks_ran) = if self.clock.is_paused() || self.sessions.is_empty() {
            self.clock.hold();
            (self.mods.open_feed(), 0)
        } else {
            self.run_fixed_ticks(dt)
        };
        let legit_motion =
            ticks_ran as f32 * crate::player::TERMINAL * crate::events::tick::TICK_DT;
        for sess in &mut self.sessions {
            let delta = (sess.player.pos - sess.replication.pos_before_ticks).length();
            sess.replication.tick_teleported = delta > 2.0 + legit_motion;
            if sess.replication.tick_teleported {
                sess.sim.fall.reset(sess.player.pos.y);
                sess.sim.pending_fall = 0.0;
            }
        }
        let mut per_session: Vec<Vec<ServerToClient>> =
            self.sessions.iter().map(|_| Vec::new()).collect();
        for pending in self.chat.take_pending() {
            for (s, out) in per_session.iter_mut().enumerate() {
                if pending.targets.includes(self.sessions[s].id) {
                    out.push(ServerToClient::ChatLine(pending.line.clone()));
                }
            }
        }
        for (s, out) in per_session.iter_mut().enumerate() {
            let sess = &mut self.sessions[s];
            let unlocked = sess.player.progression.unlocked();
            if unlocked.len() > sess.replication.sent_unlock_count {
                out.push(ServerToClient::RecipesUnlocked {
                    recipes: unlocked[sess.replication.sent_unlock_count..].to_vec(),
                });
                sess.replication.sent_unlock_count = unlocked.len();
            }
        }
        let disabled_mods = self.mods.host().disabled_count();
        for (s, out) in per_session.iter_mut().enumerate() {
            let sess = &mut self.sessions[s];
            if disabled_mods > sess.replication.sent_disabled_mods {
                out.push(ServerToClient::ModsDisabled {
                    mods: self
                        .mods
                        .host()
                        .disabled_since(sess.replication.sent_disabled_mods),
                });
                sess.replication.sent_disabled_mods = disabled_mods;
            }
        }
        let queue_room: Vec<usize> = self
            .sessions
            .iter()
            .map(|sess| {
                headroom
                    .iter()
                    .find(|(id, _)| *id == sess.id)
                    .map_or(usize::MAX, |&(_, room)| room)
            })
            .collect();
        self.pump_streaming(dt, &mut per_session, &queue_room);
        if ticks_ran > 0 {
            let feeds = self.take_window_feeds(&mut events);
            let shared = self.shared_tick_rows(&events).with_feeds(feeds);
            for (s, out) in per_session.iter_mut().enumerate() {
                if self.sessions.is_faulted(s) {
                    continue;
                }
                let update = self.isolated(s, "replication", |server| {
                    server.build_tick_update(s, &events, &shared)
                });
                if let Some(update) = update {
                    out.push(ServerToClient::Tick(Box::new(update)));
                }
            }
        }

        let mut per_session = per_session.into_iter();
        let msgs = if self.sessions.has_local_session() {
            per_session.next().expect("the local session is index 0")
        } else {
            Vec::new()
        };
        let mut remote: Vec<(PlayerId, Vec<ServerToClient>)> = self
            .sessions
            .iter()
            .skip(usize::from(self.sessions.has_local_session()))
            .map(|sess| sess.id)
            .zip(per_session)
            .collect();
        kicked.extend(self.evict_faulted());
        remote.retain(|(id, _)| kicked.iter().all(|(gone, _)| gone != id));
        self.maybe_autosave(dt);
        PumpOutput {
            msgs,
            remote,
            kicked,
        }
    }

    fn deny_unknown_slot(&mut self, s: usize, request_id: crate::net::protocol::ClientRequestId) {
        self.push_action_outcome(
            s,
            request_id,
            false,
            Some(crate::net::protocol::ActionDenyReason::Denied),
        );
    }

    pub fn apply_message(&mut self, s: usize, msg: ClientToServer) {
        match msg {
            ClientToServer::PlayerUpdate(u) => self.apply_player_update(s, &u),
            ClientToServer::Action(action) => self.apply_action(s, action),
            ClientToServer::CreativeCursor { item, request_id } => {
                self.queue_menu_action(s, PendingMenuAction::CreativeCursor { item, request_id });
            }
            ClientToServer::MenuClick {
                slot,
                button,
                shift,
                gather,
                request_id,
            } => {
                let Some(slot) = slot.to_menu_slot() else {
                    return self.deny_unknown_slot(s, request_id);
                };
                self.queue_menu_action(
                    s,
                    PendingMenuAction::SlotClick {
                        slot,
                        button: crate::net::protocol::button_from_wire(button),
                        shift,
                        gather,
                        request_id,
                    },
                );
            }
            ClientToServer::MenuSwapOffHand { slot, request_id } => {
                let Some(slot) = slot.to_menu_slot() else {
                    return self.deny_unknown_slot(s, request_id);
                };
                self.queue_menu_action(s, PendingMenuAction::SwapOffHand { slot, request_id });
            }
            ClientToServer::MenuDrag {
                slots,
                button,
                request_id,
            } => {
                let slots = slots
                    .into_iter()
                    .take(petramond_world::gui_state::MAX_MENU_DRAG_SLOTS)
                    .map(|slot| slot.to_menu_slot())
                    .collect::<Option<Vec<_>>>();
                let Some(slots) = slots else {
                    return self.deny_unknown_slot(s, request_id);
                };
                self.queue_menu_action(
                    s,
                    PendingMenuAction::SlotDrag {
                        slots,
                        button: crate::net::protocol::button_from_wire(button),
                        request_id,
                    },
                );
            }
            ClientToServer::MenuDrop {
                slot,
                all,
                request_id,
            } => {
                let Some(slot) = slot.to_menu_slot() else {
                    return self.deny_unknown_slot(s, request_id);
                };
                self.queue_menu_action(
                    s,
                    PendingMenuAction::DropSlot {
                        slot,
                        all,
                        request_id,
                    },
                );
            }
            ClientToServer::CraftRecipe {
                recipe,
                bulk,
                request_id,
            } => self.queue_menu_action(
                s,
                PendingMenuAction::CraftRecipe {
                    recipe,
                    bulk,
                    request_id,
                },
            ),
            ClientToServer::SetCraftFilter { craftable_only } => {
                self.sessions[s].player.craft_craftable_only = craftable_only;
            }
            ClientToServer::ChatSend { text } => {
                if !self.sessions[s].input.allow_chat(std::time::Instant::now()) {
                    let id = self.sessions[s].id;
                    self.chat.plain(
                        "You are sending messages too fast.",
                        crate::net::protocol::ChatColor::Red,
                        crate::server::chat::ChatTargets::Players(vec![id]),
                    );
                    return;
                }
                if text.starts_with('/') {
                    let id = self.sessions[s].id;
                    if let Some(clean) = crate::server::chat::clean_text(&text) {
                        self.execute_player_command(id, clean.strip_prefix('/').unwrap_or(""));
                    }
                } else {
                    let echo = !self.sessions.has_local_session();
                    self.chat.player(&self.sessions[s].name, &text, echo);
                }
            }
            ClientToServer::Pause(paused) => self.clock.request_pause(paused),
            ClientToServer::StreamBatchAck {
                messages_per_second,
            } => self.sessions[s]
                .transport
                .terrain
                .apply_batch_ack(messages_per_second),
            ClientToServer::SectionCacheMiss { pos } => {
                self.sessions[s].transport.terrain.handle_cache_miss(pos)
            }
            ClientToServer::SetViewDistance { chunks } => {
                self.sessions[s].transport.view_radius = (chunks as i32).clamp(4, 64);
                if s == 0 && self.sessions.has_local_session() {
                    self.world.set_render_dist((chunks as i32).clamp(4, 64));
                }
            }
            ClientToServer::KeepAlive => {}
            ClientToServer::Hello { .. }
            | ClientToServer::KeyExchange { .. }
            | ClientToServer::ModQuery
            | ClientToServer::Join { .. }
            | ClientToServer::Disconnect => {
                log::warn!("ignoring handshake/lifecycle message on a joined session");
            }
        }
    }

    fn apply_player_update(&mut self, s: usize, u: &PlayerUpdate) {
        let t = u.transform;
        if !(t.pos.is_finite()
            && t.vel.is_finite()
            && t.yaw.is_finite()
            && t.pitch.is_finite()
            && u.wishdir.is_finite())
        {
            log::warn!("dropping PlayerUpdate with non-finite transform/intent");
            return;
        }

        let sess = &mut self.sessions[s];
        sess.input.move_wishdir = u.wishdir;
        sess.input.move_jump = u.jump;
        sess.input.move_sprint = u.sprint;
        sess.input.claim_pos = t.pos;
        sess.input.claim_vel = t.vel;
        sess.input.claim_on_ground = u.on_ground;
        sess.input.claim_fresh = true;
        sess.replication.last_reported_transform = Some(SelfTransform {
            transform: t,
            on_ground: u.on_ground,
        });
        sess.player.yaw = t.yaw;
        sess.player.pitch = t.pitch;
        sess.player.inventory.set_active(u.hotbar_slot);
        let selected = sess.selected_item();
        sess.input
            .held_rotation
            .apply_wire(u.held_rotation, selected);

        sess.input.intent_gameplay = u.gameplay;
        sess.input.intent_sneak = u.sneak;
        if u.gameplay {
            sess.input.intent_break_held = u.break_held;
            sess.input.intent_use_held = u.use_held;
        } else {
            // Menu focus drops queued action edges so clicks can't fire behind screens.
            // But the dropped use click still owes an outcome, so send deny here or the client's
            // place ghost never rolls back and the ledger entry leaks.
            if let Some(id) = sess.input.drop_action_edges() {
                sess.replication
                    .push_outcome(crate::net::protocol::ActionOutcome::deny(
                        id,
                        crate::net::protocol::ActionDenyReason::Denied,
                    ));
            }
        }

        let eye = crate::server::movement::reach_eye(sess);
        sess.input.look = u
            .target
            .filter(|t| player::block_within_reach(eye, t.block));
    }
}
