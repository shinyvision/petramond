use crate::game::tick::TickEvents;
use crate::net::protocol::{ClientToServer, PlayerUpdate, SelfTransform, ServerToClient};
use crate::player;
use crate::server::player::{PendingMenuAction, PlayerId};

use super::{wire_world_events, PumpOutput, ServerGame};

impl ServerGame {
    /// [`pump_tagged`](Self::pump_tagged) for a local-only server — the
    /// synchronous test harness's pipe service; production always goes
    /// through the thread loop's `pump_tagged`.
    #[cfg(test)]
    pub(crate) fn pump(&mut self, dt: f32, inbox: &mut Vec<ClientToServer>) -> PumpOutput {
        let local = self.sessions[0].id;
        let mut tagged: Vec<(PlayerId, ClientToServer)> =
            inbox.drain(..).map(|msg| (local, msg)).collect();
        self.pump_tagged(dt, &mut tagged, &[])
    }

    /// Consume this frame's client→server messages (tagged by the sending
    /// session's `PlayerId` — resolved to indices here, so a session that
    /// left during the drain simply drops its residue), run the fixed ticks,
    /// then stream the world (gen/load around every session) and build every
    /// recipient's outbound messages — the whole per-frame server step.
    ///
    /// Per-recipient ordering contract: terrain payloads first (each column
    /// before its sections), then at most one `Tick(TickUpdate)` when a fixed
    /// tick executed. Each client applies its list in order, so a delta for a
    /// section shipped this same pump lands after its install. In-process the
    /// payloads are `Arc` refcount bumps.
    ///
    /// `headroom` is each remote connection's free outbound-queue slots
    /// (`RemoteHub::send_headroom`), keyed by `PlayerId`; the streamer paces
    /// terrain against it. A session with no entry (the local pipe, tests) is
    /// unbounded.
    pub(crate) fn pump_tagged(
        &mut self,
        dt: f32,
        inbox: &mut Vec<(PlayerId, ClientToServer)>,
        headroom: &[(PlayerId, usize)],
    ) -> PumpOutput {
        for (id, msg) in inbox.drain(..) {
            if let Some(s) = self.sessions.iter().position(|x| x.id == id) {
                self.apply_message(s, msg);
            }
        }
        // A tick that moves a session's player (bed tuck, wake, respawn, a mod
        // Teleport) is a teleport, never a fall: player physics runs only
        // client-side, so no tick-side position write can be movement. Snapshot
        // and re-anchor the fall tracker across the ticks.
        for sess in &mut self.sessions {
            sess.pos_before_ticks = sess.player.pos;
        }
        // An EMPTY session list (a headless server between players) freezes
        // the sim exactly like pause: the world clock stops, nothing spawns,
        // and — load-bearing — no tick-side `sessions[0]` mod-ABI site can
        // run against a missing session. Streaming/autosave keep pumping.
        let (mut events, ticks_ran) = if self.paused || self.sessions.is_empty() {
            // Skip the fixed ticks ONLY, and bank no tick debt — the
            // accumulator stays pinned so resume doesn't fast-forward.
            self.tick_accumulator = 0.0;
            (
                TickEvents::with_next_spatial_sound_handle(self.next_mod_sound_handle),
                0,
            )
        } else {
            self.run_fixed_ticks(dt)
        };
        for sess in &mut self.sessions {
            // Observers skip interpolating across a tick-side TELEPORT (bed,
            // respawn, mod teleport, hard knockback). Ordinary F2 physics
            // motion must still lerp — only large discontinuities snap.
            let delta = (sess.player.pos - sess.pos_before_ticks).length();
            sess.tick_teleported = delta > 2.0;
            if sess.tick_teleported {
                sess.fall.reset(sess.player.pos.y);
                sess.pending_fall = 0.0;
            }
        }
        let mut per_session: Vec<Vec<ServerToClient>> =
            self.sessions.iter().map(|_| Vec::new()).collect();
        if !self.pending_chat.is_empty() {
            for pending in self.pending_chat.drain(..) {
                for (s, out) in per_session.iter_mut().enumerate() {
                    if pending.targets.includes(self.sessions[s].id) {
                        out.push(ServerToClient::ChatLine(pending.line.clone()));
                    }
                }
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
        // AFTER the ticks, so this pump's payloads carry the tick's edits and
        // freshly-final sections ship the same frame they land.
        self.pump_streaming(dt, &mut per_session, &queue_room);
        if ticks_ran > 0 {
            // Shared batch parts built ONCE per tick window: the drained
            // world-event queue (plus any leave-path events banked between
            // ticks), the coalesced delta log, and the entity/chest rows.
            let mut world_events = std::mem::take(&mut self.pending_wire_events);
            world_events.extend(wire_world_events(&mut events.world));
            let deltas = self.world.take_block_deltas();
            let shared = self.shared_tick_rows(&events);
            for (s, out) in per_session.iter_mut().enumerate() {
                out.push(ServerToClient::Tick(Box::new(self.build_tick_update(
                    s,
                    &events,
                    &world_events,
                    &deltas,
                    &shared,
                ))));
            }
        }
        // Autosave is server-owned (wall-clock dt fed by the thread's loop);
        // it keeps running while paused.
        self.maybe_autosave(dt);

        let mut per_session = per_session.into_iter();
        let msgs = if self.has_local_session {
            per_session.next().expect("the local session is index 0")
        } else {
            Vec::new()
        };
        let remote = self
            .sessions
            .iter()
            .skip(usize::from(self.has_local_session))
            .map(|sess| sess.id)
            .zip(per_session)
            .collect();
        PumpOutput { msgs, remote }
    }

    /// Apply one message from session `s`, latching intents/edges the fixed
    /// tick consumes. Message order within a frame is preserved.
    pub(crate) fn apply_message(&mut self, s: usize, msg: ClientToServer) {
        match msg {
            ClientToServer::PlayerUpdate(u) => self.apply_player_update(s, &u),
            ClientToServer::Action(action) => self.apply_action(s, action),
            ClientToServer::MenuClick {
                slot,
                button,
                shift,
                gather,
                request_id,
            } => {
                self.sessions[s]
                    .pending_menu_actions
                    .push(PendingMenuAction::SlotClick {
                        slot: slot.to_menu_slot(),
                        button: crate::net::protocol::button_from_wire(button),
                        shift,
                        gather,
                        request_id,
                    });
            }
            ClientToServer::MenuDrag {
                slots,
                button,
                request_id,
            } => {
                let slots = slots
                    .into_iter()
                    .take(crate::gui::MAX_MENU_DRAG_SLOTS)
                    .map(|slot| slot.to_menu_slot())
                    .collect();
                self.sessions[s]
                    .pending_menu_actions
                    .push(PendingMenuAction::SlotDrag {
                        slots,
                        button: crate::net::protocol::button_from_wire(button),
                        request_id,
                    });
            }
            ClientToServer::MenuDrop {
                slot,
                all,
                request_id,
            } => self.sessions[s]
                .pending_menu_actions
                .push(PendingMenuAction::DropSlot {
                    slot: slot.to_menu_slot(),
                    all,
                    request_id,
                }),
            ClientToServer::CraftRecipe {
                recipe,
                bulk,
                request_id,
            } => self.sessions[s]
                .pending_menu_actions
                .push(PendingMenuAction::CraftRecipe {
                    recipe,
                    bulk,
                    request_id,
                }),
            ClientToServer::SetCraftFilter { craftable_only } => {
                self.sessions[s].player.craft_craftable_only = craftable_only;
            }
            ClientToServer::ChatSend { text } => {
                // A slash is a command prefix only at byte zero. Leading
                // whitespace deliberately turns it into ordinary player chat.
                if text.starts_with('/') {
                    let id = self.sessions[s].id;
                    if let Some(clean) = crate::server::chat::clean_text(&text) {
                        self.execute_player_command(id, clean.strip_prefix('/').unwrap_or(""));
                    }
                } else {
                    let name = self.sessions[s].name.clone();
                    self.enqueue_player_chat(&name, &text);
                }
            }
            // Pause is honorable only while the sole connection has always
            // been the local one. Once the server has been open to LAN the
            // gate is permanent — see the `lan_ever_opened` field.
            ClientToServer::Pause(paused) => {
                if !self.lan_ever_opened {
                    self.paused = paused;
                }
            }
            ClientToServer::StreamBatchAck {
                messages_per_second,
            } => self.sessions[s]
                .terrain
                .apply_batch_ack(messages_per_second),
            ClientToServer::SectionCacheMiss { pos } => {
                self.sessions[s].terrain.handle_cache_miss(pos)
            }
            ClientToServer::SetViewDistance { chunks } => {
                self.sessions[s].view_radius = (chunks as i32).clamp(4, 64);
                // The HOST's own slider also moves the server budget — its
                // machine runs the world, so its setting IS the server
                // setting. Remote requests only shrink under the budget
                // (streaming clamps per anchor).
                if s == 0 && self.has_local_session {
                    self.world.set_render_dist((chunks as i32).clamp(4, 64));
                }
            }
            ClientToServer::KeepAlive => {}
            // Handshake/lifecycle messages are consumed by the transport
            // (the hub's pre-join state machine and its leave path); one
            // reaching a joined session is protocol misuse.
            ClientToServer::Hello { .. }
            | ClientToServer::ModQuery
            | ClientToServer::Join { .. }
            | ClientToServer::Disconnect => {
                log::warn!("ignoring handshake/lifecycle message on a joined session");
            }
        }
    }

    /// Latch a `PlayerUpdate`: movement intent (F2), validated transform (F1),
    /// hotbar + held rotation, held intents (menu focus forces them off and
    /// drops queued edges, exactly as the old `capture_intent` did), the
    /// reach-validated look target, and the fall tracker.
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
        sess.move_wishdir = u.wishdir;
        sess.move_jump = u.jump;
        sess.move_sprint = u.sprint;
        sess.claim_pos = t.pos;
        sess.claim_vel = t.vel;
        sess.claim_on_ground = u.on_ground;
        sess.claim_fresh = true;
        // What the client last claimed — after the ticks, a session transform
        // that drifted from this means the server rejected the claim or a
        // tick-side teleport/knockback moved the player.
        sess.last_reported_transform = Some(SelfTransform {
            transform: t,
            on_ground: u.on_ground,
        });
        sess.player.yaw = t.yaw;
        sess.player.pitch = t.pitch;
        sess.player.inventory.set_active(u.hotbar_slot);
        let selected = sess.selected_item();
        sess.held_rotation.apply_wire(u.held_rotation, selected);

        sess.intent_gameplay = u.gameplay;
        sess.intent_sneak = u.sneak;
        if u.gameplay {
            sess.intent_break_held = u.break_held;
            sess.intent_use_held = u.use_held;
        } else {
            // Menu focus drops queued action edges so clicks cannot fire behind screens.
            sess.intent_break_held = false;
            sess.intent_use_held = false;
            sess.pending_attack = false;
            sess.pending_attack_mob = None;
            sess.pending_attack_player = None;
            // The dropped click still owes its outcome: deny, so the client
            // rolls its place ghost back instead of leaking the ledger entry.
            if let Some(id) = sess
                .pending_use_click
                .take()
                .and_then(|click| click.request_id)
            {
                sess.pending_action_outcomes.push(crate::game::prediction::deny(
                    id,
                    crate::net::protocol::ActionDenyReason::Denied,
                ));
            }
        }

        // Reach validation against the claimed eye, BOUNDED by the F1 drift
        // ring (`movement::reach_eye`) — an implausible claim must not grant
        // remote reach. Measured to the CLOSEST point of the cell.
        let eye = crate::server::movement::reach_eye(sess);
        sess.look = u
            .target
            .filter(|t| player::block_within_reach(eye, t.block));
    }
}
