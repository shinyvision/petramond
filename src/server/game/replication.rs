use crate::game::tick::{TickEvents, WorldEvents};
use crate::mathh::IVec3;
use crate::net::protocol::{
    BlockDelta, ItemSlotWire, ItemStateRow, MobStateRow, ModSpatialSoundMsg, OpenScreen,
    PlayerActionKind, PlayerStateRow, SelfEvents, SelfState, SelfTransform, TickUpdate, Transform,
    WorldEventMsg,
};

use super::{ServerGame, SharedTickRows};

impl ServerGame {
    /// The per-tick batch parts every recipient shares, built once per window
    /// and cheaply cloned per recipient (small rows; `SectionBytes` never
    /// rides here). `&mut` for the env-diff bookkeeping only.
    pub(crate) fn shared_tick_rows(&mut self, events: &TickEvents) -> SharedTickRows {
        let mobs = self
            .world
            .mobs()
            .instances()
            .iter()
            .map(|m| MobStateRow {
                id: m.id(),
                kind_id: m.kind.0,
                pos: m.pos,
                yaw: m.yaw,
                anim_time: m.anim_time,
                moving: m.moving,
                idle_anim: m.idle_anim,
                head_yaw: m.head_yaw,
                head_pitch: m.head_pitch,
                hurt_timer: m.hurt_timer(),
                dead: m.is_dead(),
                shorn: m.is_shorn(),
                emitters: m.active_emitters().to_vec(),
                anims: m
                    .active_anims()
                    .iter()
                    .map(|l| (l.name.clone(), l.phase))
                    .collect(),
                // Present only while the death ragdoll plays (bounded): the
                // pose as of this tick (alpha 1.0); the client interpolates
                // consecutive batches.
                ragdoll: m.ragdoll_pose(1.0).map(|pose| {
                    pose.into_iter()
                        .map(|(p, q)| (p.to_array(), q.to_array()))
                        .collect()
                }),
            })
            .collect();
        let items = self
            .world
            .item_entities()
            .iter()
            .map(|it| ItemStateRow {
                id: it.id,
                item_id: it.stack.item.0,
                count: it.stack.count,
                pos: it.pos,
                spin: it.spin,
            })
            .collect();
        // Player rows: EVERY session, to every recipient (the client skips
        // its own id). `hurt_recent` ships the damage EDGE — sessions track
        // no hurt timer; each client runs its own flash envelope.
        let players = self
            .sessions
            .iter()
            .enumerate()
            .map(|(s, sess)| {
                let alive = sess.player.health() > 0;
                PlayerStateRow {
                    id: sess.id,
                    transform: Transform {
                        pos: sess.player.pos,
                        vel: sess.player.vel,
                        yaw: sess.player.yaw,
                        pitch: sess.player.pitch,
                    },
                    on_ground: sess.player.on_ground,
                    sneaking: sess.sneaking(),
                    sleeping: sess.sleep.is_some(),
                    sleep_yaw: self.sleep_head_yaw(s),
                    alive,
                    visible: alive && !sess.player.is_spectator(),
                    held_item: sess.selected_item().map(|item| item.0),
                    // The same overlay state `SelfState::mining` ships for the
                    // player's own hand: target cell + crack stage. Observers
                    // derive the arm-swing flag AND the remote crack overlay.
                    mining: sess.mining.overlay(),
                    eating: sess.eating.is_some(),
                    hurt_recent: events.player_at(s).player_damaged,
                    snap: sess.tick_teleported,
                    mount: sess.mount.map(crate::net::protocol::PlayerMount::from_mount),
                }
            })
            .collect();
        let mut player_actions = Vec::new();
        for (s, sess) in self.sessions.iter().enumerate() {
            let p = events.player_at(s);
            let mut push = |kind| player_actions.push((sess.id, kind));
            if p.swung_hand {
                push(PlayerActionKind::Swung);
            }
            if p.broke_block.is_some() {
                push(PlayerActionKind::Broke);
            }
            if p.placed_block.is_some() {
                push(PlayerActionKind::Placed);
            }
            if p.threw_item {
                push(PlayerActionKind::ThrewItem);
            }
            if p.used_item {
                push(PlayerActionKind::UsedItem);
            }
            if p.interacted {
                push(PlayerActionKind::Interacted);
            }
            if p.ate_finished {
                push(PlayerActionKind::AteFinished);
            }
            if p.player_died {
                push(PlayerActionKind::Died);
            }
            if p.respawned {
                push(PlayerActionKind::Respawned);
            }
        }
        // Full open-chest state per batch (chest_viewers keys; tiny), sorted
        // so the wire batch is deterministic.
        let mut open_chests: Vec<IVec3> = self.chest_viewers.keys().copied().collect();
        open_chests.sort_unstable_by_key(|p| (p.x, p.y, p.z));
        // Environment shader params: value-compare against the last-shipped
        // copy and ship the changed FULL set (the map is ~a dozen entries;
        // day/night rewrites its params most ticks, so this rides most
        // windows). `None` = unchanged, the client keeps what it has.
        let params = self.world.environment().shader_params().clone();
        let env = if self
            .last_shipped_env
            .as_ref()
            .is_some_and(|last| **last == *params)
        {
            None
        } else {
            let rows = params.iter().map(|(k, v)| (k.clone(), *v)).collect();
            self.last_shipped_env = Some(params);
            Some(rows)
        };
        SharedTickRows {
            tick: self.world.current_tick(),
            clock: crate::server::daynight::current_clock(&self.world),
            mobs,
            items,
            players,
            player_actions,
            open_chests,
            env,
        }
    }

    /// Build one recipient's replication batch: the shared rows (all mobs and
    /// dropped items as of the latest tick — interest scoping lands with
    /// per-player streaming), the window's coalesced block deltas restricted
    /// to the recipient's sent sections, the window's world events + session
    /// `s`'s one-shots, its menu sync (when changed), and its own state.
    pub(crate) fn build_tick_update(
        &mut self,
        s: usize,
        events: &TickEvents,
        world_events: &[WorldEventMsg],
        deltas: &[BlockDelta],
        shared: &SharedTickRows,
    ) -> TickUpdate {
        // Per-recipient delta filter: only sections this client holds.
        let mut block_deltas: Vec<BlockDelta> = deltas
            .iter()
            .filter(|d| self.sessions[s].terrain.covers(d.pos))
            .copied()
            .collect();
        // Corrective cell sync: the CURRENT state of cells a use click
        // disagreed about (no-op click, denied place) — how a client whose
        // replica lied (ghost block, stale cell) reconciles. A shared delta
        // for the same cell already carries the truth.
        for pos in std::mem::take(&mut self.sessions[s].pending_corrective_cells) {
            if !self.sessions[s].terrain.covers(pos) || block_deltas.iter().any(|d| d.pos == pos) {
                continue;
            }
            if let Some(d) = self.world.block_delta_at(pos) {
                block_deltas.push(d);
            }
        }
        let action_outcomes = std::mem::take(&mut self.sessions[s].pending_action_outcomes);
        // Echo rule: the initiator already presented their own place/break
        // locally — strip matching world events from THEIR batch only.
        // Observers still receive the shared list unchanged.
        let presented_places: rustc_hash::FxHashSet<_> =
            std::mem::take(&mut self.sessions[s].presented_places)
                .into_iter()
                .collect();
        let presented_breaks: rustc_hash::FxHashSet<_> =
            std::mem::take(&mut self.sessions[s].presented_breaks)
                .into_iter()
                .collect();
        let events_for_recipient: Vec<WorldEventMsg> =
            if presented_places.is_empty() && presented_breaks.is_empty() {
                world_events.to_vec()
            } else {
                world_events
                    .iter()
                    .filter(|ev| match ev {
                        WorldEventMsg::BlockPlaced { pos, .. } => !presented_places.contains(pos),
                        WorldEventMsg::BlockBroken { pos, .. } => !presented_breaks.contains(pos),
                        _ => true,
                    })
                    .cloned()
                    .collect()
            };
        TickUpdate {
            tick: shared.tick,
            clock: shared.clock,
            block_deltas,
            mobs: shared.mobs.clone(),
            items: shared.items.clone(),
            players: shared.players.clone(),
            player_actions: shared.player_actions.clone(),
            self_state: Some(self.build_self_state(s)),
            open_chests: shared.open_chests.clone(),
            env: shared.env.clone(),
            events: events_for_recipient,
            self_events: self.build_self_events(s, events),
            action_outcomes,
            menu_sync: self.build_menu_sync(s),
        }
    }

    /// Session `s`'s per-tick one-shots: the lossy `PlayerTickEvents` slice
    /// plus the session's screen-request outbox (taken here — the request
    /// fields are internal; the client only sees `OpenScreen`).
    fn build_self_events(&mut self, s: usize, events: &TickEvents) -> SelfEvents {
        let p = events.player_at(s);
        let sess = &mut self.sessions[s];
        // Take every request so nothing lingers; the tick can only set one of
        // them (one consumed click per tick), so first-Some is the open.
        let gui = sess.request_open_gui.take();
        let sleep = std::mem::take(&mut sess.request_open_sleep);
        let open_screen = if let Some((kind, pos)) = gui {
            // The wire speaks kind KEYS (GuiKind ids are process-local) — one
            // lane for engine containers and mod GUIs alike.
            crate::gui::kind_key(kind).map(|kind_key| OpenScreen::Gui {
                kind_key: kind_key.to_string(),
                pos,
            })
        } else if sleep {
            Some(OpenScreen::Sleep)
        } else {
            None
        };
        // The hand one-shots (broke/placed/swung/threw/used/interacted) are
        // deliberately absent: the recipient initiated them and animated at
        // click time echo rule. Observers
        // get them via the shared `player_actions` rows.
        SelfEvents {
            picked_up_item: p.picked_up_item,
            bed_interacted: p.bed_interacted,
            player_damaged: p.player_damaged,
            player_died: p.player_died,
            sleep_ended: p.sleep_ended,
            respawned: p.respawned,
            open_screen,
            close_mod_gui: std::mem::take(&mut sess.request_close_mod_gui),
            toggled_door: p.toggled_door,
            used_unpredicted: p.used_unpredicted,
        }
    }

    /// Session `s`'s own replicated state. The full inventory rides only when
    /// its revision moved since the last state this session was sent (always
    /// on the first update after join).
    pub(crate) fn build_self_state(&mut self, s: usize) -> SelfState {
        let sleeping = self.sleep_progress01(s).map(|p| (p * 255.0).round() as u8);
        let sleep_bed = self.sleep_bed_base(s);
        let sess = &mut self.sessions[s];
        let player = &sess.player;
        // Transform correction: ships only on REAL divergence from the last
        // CLIENT-REPORTED transform — a rejected claim, a tick-side
        // teleport/knockback, or the very first update (nothing reported yet;
        // the echo is a no-op there, the client seeded from the same player).
        // The server free-runs its integration past a slow client's last
        // claim, so plain inequality is just time-phase drift — correcting it
        // rubber-bands the client. The deadbands scale with the claim gap.
        let current = SelfTransform {
            transform: Transform {
                pos: player.pos,
                vel: player.vel,
                yaw: player.yaw,
                pitch: player.pitch,
            },
            on_ground: player.on_ground,
        };
        let diverged = match &sess.last_reported_transform {
            None => true,
            Some(r) => {
                let spectator = player.is_spectator();
                let gap = sess.ticks_since_claim;
                let r = &r.transform;
                !crate::server::movement::claim_within_drift(spectator, gap, player.pos - r.pos)
                    || (player.vel - r.vel).length()
                        > crate::server::movement::vel_correction_eps(gap)
                    // Yaw/pitch never extrapolate (ticks don't turn the
                    // head), so any difference is a genuine server-side set.
                    || player.yaw != r.yaw
                    || player.pitch != r.pitch
            }
        };
        // A mounted player never receives corrections: the server slaves them
        // to the mount every tick (always "diverged" from the claim), and the
        // client slaves itself to the same replicated mount row. The dismount
        // tick clears `mount`, so the first free tick corrects any residue.
        let transform = (sess.mount.is_none() && diverged).then_some(current);
        let revision = player.inventory.revision();
        let inventory = (sess.last_sent_inventory_revision != Some(revision)).then(|| {
            player
                .inventory
                .raw_slots()
                .iter()
                .copied()
                .chain(std::iter::once(player.inventory.cursor().copied()))
                .map(|slot| {
                    slot.map(|st| ItemSlotWire {
                        item_id: st.item.0,
                        count: st.count,
                    })
                })
                .collect()
        });
        sess.last_sent_inventory_revision = Some(revision);
        SelfState {
            health: player.health(),
            mode: match player.mode() {
                crate::player::PlayerMode::Survival => 0,
                crate::player::PlayerMode::Spectator => 1,
            },
            effects: player
                .effects()
                .iter()
                .map(|e| (e.effect.0, e.remaining))
                .collect(),
            inventory_revision: revision,
            inventory,
            eating: sess
                .eating_progress()
                .map(|p| (p.clamp(0.0, 1.0) * 255.0).round() as u8),
            sleeping,
            sleep_bed,
            transform,
        }
    }

    /// Session `s`'s menu-session view when it changed since the last sync
    /// this session was sent (`None` = unchanged). The base message compares
    /// by value (its `gui_state` field is held `None` for the compare); the
    /// state map compares by `Arc` identity — holding the shipped `Arc` in
    /// `last_sent_gui_state` forces the next tick-side write to copy-on-write
    /// onto a fresh allocation, which is what makes identity sound here.
    pub(crate) fn build_menu_sync(
        &mut self,
        s: usize,
    ) -> Option<crate::net::protocol::MenuSyncMsg> {
        use crate::net::protocol::{GuiValueWire, MenuTargetWire};

        let base = self.build_menu_sync_base(s);
        let target = self.sessions[s].menu.target();
        let gui_arc = target
            .kind()
            .is_some_and(|kind| kind.is_mod())
            .then(|| self.mod_gui_state_for_menu_sync(s, target));
        let sess = &mut self.sessions[s];
        let gui_changed = match (&gui_arc, &sess.last_sent_gui_state) {
            (None, None) => false,
            (Some(a), Some(b)) => !std::sync::Arc::ptr_eq(a, b),
            _ => true,
        };
        let base_changed = sess.last_menu_sync.as_ref() != Some(&base);
        if !base_changed && !gui_changed {
            return None;
        }
        let mut out = base.clone();
        if gui_changed {
            if let (MenuTargetWire::ModGui { gui_state, .. }, Some(map)) =
                (&mut out.target, &gui_arc)
            {
                *gui_state = Some(
                    map.iter()
                        .map(|(k, v)| (k.clone(), GuiValueWire::from_value(v)))
                        .collect(),
                );
            }
            sess.last_sent_gui_state = gui_arc;
        }
        sess.last_menu_sync = Some(base);
        Some(out)
    }

    fn mod_gui_state_for_menu_sync(
        &self,
        s: usize,
        target: crate::game::container::ContainerTarget,
    ) -> std::sync::Arc<crate::gui::GuiStateMap> {
        let own = self.sessions[s].gui_state.clone();
        if s == 0 || !own.is_empty() {
            return own;
        }
        let host = self.sessions[0].gui_state.clone();
        if host.is_empty() || !self.host_mod_gui_state_applies_to(target) {
            return own;
        }
        host
    }

    fn host_mod_gui_state_applies_to(
        &self,
        target: crate::game::container::ContainerTarget,
    ) -> bool {
        if self.sessions[0].menu.target() == target {
            return true;
        }
        let mut open_target = None;
        for sess in &self.sessions {
            let t = sess.menu.target();
            if !t.kind().is_some_and(|kind| kind.is_mod()) {
                continue;
            }
            if open_target.is_some_and(|seen| seen != t) {
                return false;
            }
            open_target = Some(t);
        }
        open_target == Some(target)
    }
}

/// Drain one tick window's world-anchored event queues into the wire batch
/// every recipient shares. Order: block/door/chest/pickup events first (they
/// key presentation state seeds), then the sound queues, each in emission
/// order.
pub(crate) fn wire_world_events(world: &mut WorldEvents) -> Vec<WorldEventMsg> {
    let mut out = Vec::new();
    for ev in world.block_broken.drain(..) {
        out.push(WorldEventMsg::BlockBroken {
            pos: ev.pos,
            block_id: ev.block.0,
            normal: ev.normal,
        });
    }
    for (pos, block) in world.block_placed.drain(..) {
        out.push(WorldEventMsg::BlockPlaced {
            pos,
            block_id: block.0,
        });
    }
    for (lower, open) in world.door_changed.drain(..) {
        out.push(WorldEventMsg::DoorToggled { lower, open });
    }
    for (pos, open) in world.chest_changed.drain(..) {
        out.push(if open {
            WorldEventMsg::ChestOpened { pos }
        } else {
            WorldEventMsg::ChestClosed { pos }
        });
    }
    for (pos, by) in world.item_picked_up.drain(..) {
        out.push(WorldEventMsg::ItemPickedUp { pos, by });
    }
    for s in world.mob_sounds.drain(..) {
        out.push(WorldEventMsg::MobSound {
            mob_id: s.mob_id,
            kind_id: s.kind.0,
            category: s.category.to_u8(),
            pos: s.pos,
        });
    }
    for (emitter_id, pos, intensity) in world.emitter_bursts.drain(..) {
        out.push(WorldEventMsg::EmitterBurst {
            emitter_id,
            pos,
            intensity,
        });
    }
    for s in world.sounds.drain(..) {
        out.push(WorldEventMsg::ModSound {
            sound_id: s.sound.0,
            pos: s.pos,
        });
    }
    for c in world.spatial_sounds.drain(..) {
        use crate::game::ModSpatialSoundCommand as Cmd;
        out.push(WorldEventMsg::ModSpatialSound(match c {
            Cmd::PlayAt {
                handle,
                sound,
                pos,
                volume,
                pitch,
            } => ModSpatialSoundMsg::PlayAt {
                handle,
                sound_id: sound.0,
                pos,
                volume,
                pitch,
            },
            Cmd::PlayOnMob {
                handle,
                sound,
                mob_id,
                volume,
                pitch,
                last_pos,
            } => ModSpatialSoundMsg::PlayOnMob {
                handle,
                sound_id: sound.0,
                mob_id,
                volume,
                pitch,
                last_pos,
            },
            Cmd::Stop { handle } => ModSpatialSoundMsg::Stop { handle },
        }));
    }
    out
}
