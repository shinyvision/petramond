use crate::events::tick::{TickEvents, WorldEvents};
use crate::net::protocol::{
    BlockDelta, ClientEventMsg, ItemSlotWire, OpenScreen, SelfEvents, SelfState, SelfTransform,
    SleepTally, SpatialSoundMsg, TickUpdate, Transform, WorldEventMsg,
};
use petramond_world::inventory::Hand;

use super::spatial_loops::LiveSpatialLoops;
use super::{ServerGame, SharedTickRows};

/// The server-wide replication bookkeeping every recipient's batch draws on
/// (per-recipient bookkeeping lives on each session). Replication state, not
/// sim state.
#[derive(Default)]
pub struct Broadcast {
    /// World-anchored wire events produced OUTSIDE a tick window (a leaving
    /// session's menu close, e.g. its chest 1→0 transition), shipped with the
    /// next executed tick's batch so no observer misses them.
    pending_wire_events: Vec<WorldEventMsg>,
    /// Every spatial LOOP still playing (a `loop` row started and not yet
    /// stopped), by handle — replayed to a joining session, ended with its
    /// mob. See [`super::spatial_loops`].
    live_spatial_loops: LiveSpatialLoops,
    /// The `WorldEnvironment` shader-param map the last `TickUpdate.env`
    /// shipped (value-compared per tick window; the map is tiny). `None` =
    /// nothing shipped yet, so the next window carries the full set.
    last_shipped_env: Option<std::sync::Arc<crate::world::environment::ShaderParamMap>>,
}

impl Broadcast {
    /// Bank world events produced between tick windows for the next batch.
    pub fn bank_wire_events(&mut self, events: impl IntoIterator<Item = WorldEventMsg>) {
        self.pending_wire_events.extend(events);
    }

    /// The banked out-of-window events, oldest first, for the batch about to
    /// ship.
    pub fn take_wire_events(&mut self) -> Vec<WorldEventMsg> {
        std::mem::take(&mut self.pending_wire_events)
    }

    /// Forget what the last batch shipped of the environment, so the next
    /// window carries the full set — a joining session must receive the
    /// CURRENT params even when the map is static (a frozen clock freezes
    /// day/night AND weather params).
    pub fn reseed_env(&mut self) {
        self.last_shipped_env = None;
    }

    /// The full shader-param map when `params` differs from what the last
    /// batch shipped (then remembered as shipped), else `None`.
    pub fn env_update(
        &mut self,
        params: std::sync::Arc<crate::world::environment::ShaderParamMap>,
    ) -> Option<Vec<(String, [f32; 4])>> {
        if self
            .last_shipped_env
            .as_ref()
            .is_some_and(|last| **last == *params)
        {
            return None;
        }
        let rows = params.iter().map(|(k, v)| (k.clone(), *v)).collect();
        self.last_shipped_env = Some(params);
        Some(rows)
    }

    pub fn spatial_loops(&self) -> &LiveSpatialLoops {
        &self.live_spatial_loops
    }

    pub fn spatial_loops_mut(&mut self) -> &mut LiveSpatialLoops {
        &mut self.live_spatial_loops
    }
}

impl ServerGame {
    /// The per-tick batch parts, built once per window: the parts every
    /// recipient shares, and each recipient's entity lanes — which advance
    /// every session's interest set, so every session must be sent this
    /// window's batch. Entity rows are built once and shared across the
    /// lanes that select them (`SectionBytes` never rides here).
    pub fn shared_tick_rows(&mut self, events: &TickEvents) -> SharedTickRows {
        let recipients = self.entity_lanes(events);
        let sleep_tally = SleepTally {
            sleeping: self.sessions.iter().filter(|s| s.sim.sleep.is_some()).count() as u16,
            connected: self.sessions.len() as u16,
        };
        // Full open-chest state per batch (tiny), sorted so the wire batch is
        // deterministic.
        let open_chests = self.containers.open_chests();
        // Environment shader params: value-compare against the last-shipped
        // copy and ship the changed FULL set (the map is ~a dozen entries;
        // day/night rewrites its params most ticks, so this rides most
        // windows). `None` = unchanged, the client keeps what it has.
        let env = self
            .broadcast
            .env_update(self.world.data().environment().shader_params().clone());
        SharedTickRows {
            tick: self.world.current_tick(),
            clock: crate::server::daynight::current_clock(&self.world),
            recipients,
            sleep_tally,
            open_chests,
            env,
        }
    }

    /// Build one recipient's replication batch: its entity lanes (the mobs,
    /// dropped items and players in its interest as of the latest tick), the
    /// window's coalesced block deltas restricted to the recipient's sent
    /// sections, the window's world events + session
    /// `s`'s one-shots, its menu sync (when changed), and its own state.
    #[allow(clippy::too_many_arguments)]
    pub fn build_tick_update(
        &mut self,
        s: usize,
        events: &TickEvents,
        world_events: &[WorldEventMsg],
        deltas: &[BlockDelta],
        kv_deltas: &[crate::net::protocol::CellKvDelta],
        draw_deltas: &[crate::net::protocol::BlockDrawDelta],
        shared: &SharedTickRows,
    ) -> TickUpdate {
        let entities = &shared.recipients[s];
        // Per-recipient delta filter: only sections this client holds.
        let mut block_deltas: Vec<BlockDelta> = deltas
            .iter()
            .filter(|d| self.sessions[s].transport.terrain.covers(d.pos))
            .cloned()
            .collect();
        // Cell KV deltas ride the same recipient filter as block deltas.
        let cell_kv_deltas: Vec<crate::net::protocol::CellKvDelta> = kv_deltas
            .iter()
            .filter(|d| self.sessions[s].transport.terrain.covers(d.pos))
            .cloned()
            .collect();
        // Mod draw sets ride the same recipient filter.
        let block_draws: Vec<crate::net::protocol::BlockDrawDelta> = draw_deltas
            .iter()
            .filter(|d| self.sessions[s].transport.terrain.covers(d.pos))
            .cloned()
            .collect();
        // Corrective cell sync: the CURRENT state of cells a use click
        // disagreed about (no-op click, denied place) — how a client whose
        // replica lied (ghost block, stale cell) reconciles. A shared delta
        // for the same cell already carries the truth.
        for pos in std::mem::take(&mut self.sessions[s].replication.pending_corrective_cells) {
            if !self.sessions[s].transport.terrain.covers(pos) || block_deltas.iter().any(|d| d.pos == pos) {
                continue;
            }
            if let Some(d) = self.world.block_delta_at(pos) {
                block_deltas.push(d);
            }
        }
        let action_outcomes = std::mem::take(&mut self.sessions[s].replication.pending_action_outcomes);
        // Echo rule: the initiator already presented their own place/break
        // locally — strip matching world events from THEIR batch only.
        // Observers still receive the shared list unchanged.
        let presented_places: rustc_hash::FxHashSet<_> =
            std::mem::take(&mut self.sessions[s].replication.presented_places)
                .into_iter()
                .collect();
        let presented_breaks: rustc_hash::FxHashSet<_> =
            std::mem::take(&mut self.sessions[s].replication.presented_breaks)
                .into_iter()
                .collect();
        // The recipient's own catch-up events lead, so a replayed loop's
        // start precedes any retune or stop the shared window carries.
        let mut events_for_recipient: Vec<WorldEventMsg> =
            std::mem::take(&mut self.sessions[s].replication.pending_world_events);
        if presented_places.is_empty() && presented_breaks.is_empty() {
            events_for_recipient.extend_from_slice(world_events);
        } else {
            events_for_recipient.extend(
                world_events
                    .iter()
                    .filter(|ev| match ev {
                        WorldEventMsg::BlockPlaced { pos, .. } => !presented_places.contains(pos),
                        WorldEventMsg::BlockBroken { pos, .. } => !presented_breaks.contains(pos),
                        _ => true,
                    })
                    .cloned(),
            );
        }
        TickUpdate {
            block_draws,
            tick: shared.tick,
            clock: shared.clock,
            block_deltas,
            cell_kv_deltas,
            // Refcount bumps plus index lists, not deep copies: see
            // `TickUpdate::mobs`.
            mobs: entities.mobs.clone(),
            items: entities.items.clone(),
            players: entities.players.clone(),
            player_actions: std::sync::Arc::clone(&entities.player_actions),
            sleep_tally: shared.sleep_tally,
            self_state: Some(self.build_self_state(s)),
            open_chests: shared.open_chests.clone(),
            env: shared.env.clone(),
            events: events_for_recipient,
            self_events: self.build_self_events(s, events),
            action_outcomes,
            creative: self.sessions[s].sim.creative.take_replies(),
            schematics: self.sessions[s].sim.schematic.take_notices(),
            menu_sync: self.build_menu_sync(s),
        }
    }

    /// Session `s`'s per-tick one-shots: the lossy `PlayerTickEvents` slice
    /// plus the session's screen-request outbox (taken here — the request
    /// fields are internal; the client only sees `OpenScreen`).
    fn build_self_events(&mut self, s: usize, events: &TickEvents) -> SelfEvents {
        let p = events.player_at(s);
        let sess = &mut self.sessions[s];
        let id = sess.id;
        // Take every request so nothing lingers; the tick can only set one of
        // them (one consumed click per tick), so first-Some is the open.
        let gui = sess.replication.request_open_gui.take();
        let sleep = std::mem::take(&mut sess.replication.request_open_sleep);
        let open_screen = if let Some((kind, anchor)) = gui {
            // The wire speaks kind KEYS (GuiKind ids are process-local) — one
            // lane for engine containers and mod GUIs alike.
            petramond_world::gui_state::kind_key(kind).map(|kind_key| OpenScreen::Gui {
                kind_key: kind_key.to_string(),
                anchor,
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
            animator_events: p.animator_events.clone(),
            picked_up_item: p.picked_up_item,
            bed_interacted: p.bed_interacted,
            player_damaged: p.player_damaged,
            player_died: p.player_died,
            sleep_ended: p.sleep_ended,
            respawned: p.respawned,
            open_screen,
            close_document_gui: std::mem::take(&mut sess.replication.request_close_gui),
            toggled_panel: p.toggled_panel,
            used_unpredicted: p.used_unpredicted,
            used_unpredicted_off: p.used_unpredicted && p.click_off_hand,
            // Addressed by player id rather than by session index: a mod names
            // the recipient it means, and the queue is shared across sessions.
            client_events: events
                .client_events
                .iter()
                .filter(|ev| ev.player == id)
                .map(|ev| ClientEventMsg {
                    key: ev.key.clone(),
                    data: ev.data.clone(),
                })
                .collect(),
        }
    }

    /// Session `s`'s own replicated state. The full inventory rides only when
    /// its revision moved since the last state this session was sent (always
    /// on the first update after join).
    pub fn build_self_state(&mut self, s: usize) -> SelfState {
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
        let diverged = match &sess.replication.last_reported_transform {
            None => true,
            Some(r) => {
                let spectator = player.is_spectator();
                let gap = sess.input.ticks_since_claim;
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
        let transform = (sess.sim.mount.is_none() && diverged).then_some(current);
        let revision = player.inventory.revision();
        let inventory = (sess.replication.last_sent_inventory_revision != Some(revision)).then(|| {
            player
                .inventory
                .raw_slots()
                .iter()
                .copied()
                .chain(std::iter::once(player.inventory.cursor().copied()))
                .chain(std::iter::once(player.inventory.off_hand().copied()))
                .map(|slot| slot.map(ItemSlotWire::from_stack))
                .collect()
        });
        sess.replication.last_sent_inventory_revision = Some(revision);
        SelfState {
            health: player.health(),
            conditions: condition_stages(player.conditions()),
            mode: player.mode().to_u8(),
            effects: player
                .effects()
                .iter()
                .map(|e| (e.effect.0, e.remaining))
                .collect(),
            held_pose_main: player.claims.held_pose(Hand::Main),
            held_pose_off: player.claims.held_pose(Hand::Off),
            held_display: [
                player.claims.held_display(Hand::Main).map(|i| i.0),
                player.claims.held_display(Hand::Off).map(|i| i.0),
            ],
            bone_poses: player.claims.bone_poses().collect(),
            animator: player.claims.animator().clone(),
            inventory_revision: revision,
            inventory,
            eating: sess
                .eating_progress()
                .map(|p| (p.clamp(0.0, 1.0) * 255.0).round() as u8),
            eating_off_hand: sess
                .sim.eating
                .as_ref()
                .is_some_and(|eat| eat.hand == petramond_world::inventory::Hand::Off),
            sleeping,
            sleep_bed,
            // Only the half the recipient cannot work out for itself: it folds
            // the engine's own claim from its own effect list, mode and menu,
            // so sending that half too would double it.
            move_scale: player
                .claims
                .replicated_attribute(mod_api::PlayerAttribute::MoveSpeed),
            denied_actions: player.claims.replicated_denied_actions(),
            transform,
        }
    }

    /// Session `s`'s menu-session view when it changed since the last sync
    /// this session was sent (`None` = unchanged). The base message compares
    /// by value (its `gui_state` field is held `None` for the compare); the
    /// state map compares by `Arc` identity — holding the shipped `Arc` in
    /// `last_sent_gui_state` forces the next tick-side write to copy-on-write
    /// onto a fresh allocation, which is what makes identity sound here.
    pub fn build_menu_sync(&mut self, s: usize) -> Option<crate::net::protocol::MenuSyncMsg> {
        use crate::net::protocol::{GuiValueWire, MenuTargetWire};

        let base = self.build_menu_sync_base(s);
        let target = self.sessions[s].sim.menu.target();
        let gui_arc = target
            .kind()
            .is_some_and(|kind| kind.is_registered())
            .then(|| self.gui_state_for_menu_sync(s, target));
        let sess = &mut self.sessions[s];
        // Pointer identity first (the no-publish fast path), then VALUE
        // equality: a machine that re-publishes identical readings every tick
        // (the anvil's ~20 keys, every gauge) forces `Arc::make_mut` onto a
        // fresh allocation each time — ptr inequality alone re-shipped the
        // whole map 20×/s per viewer for zero information.
        let gui_changed = match (&gui_arc, &sess.replication.last_sent_gui_state) {
            (None, None) => false,
            (Some(a), Some(b)) => !std::sync::Arc::ptr_eq(a, b) && **a != **b,
            _ => true,
        };
        let base_changed = sess.replication.last_menu_sync.as_ref() != Some(&base);
        if !base_changed && !gui_changed {
            return None;
        }
        let mut out = base.clone();
        if gui_changed {
            if let (MenuTargetWire::Container { gui_state, .. }, Some(map)) =
                (&mut out.target, &gui_arc)
            {
                *gui_state = Some(
                    map.iter()
                        .map(|(k, v)| (k.clone(), GuiValueWire::from_value(v)))
                        .collect(),
                );
            }
            sess.replication.last_sent_gui_state = gui_arc;
        }
        sess.replication.last_menu_sync = Some(base);
        Some(out)
    }

    fn gui_state_for_menu_sync(
        &self,
        s: usize,
        target: crate::menu::ContainerTarget,
    ) -> std::sync::Arc<petramond_world::gui_state::GuiStateMap> {
        let own = self.sessions[s].sim.gui_state.clone();
        if s == 0 || !own.is_empty() {
            return own;
        }
        let host = self.sessions[0].sim.gui_state.clone();
        if host.is_empty() || !self.host_gui_state_applies_to(target) {
            return own;
        }
        host
    }

    fn host_gui_state_applies_to(&self, target: crate::menu::ContainerTarget) -> bool {
        if self.sessions[0].sim.menu.target() == target {
            return true;
        }
        let mut open_target = None;
        for sess in &self.sessions {
            let t = sess.sim.menu.target();
            if !t.kind().is_some_and(|kind| kind.is_registered()) {
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
pub fn wire_world_events(world: &mut WorldEvents) -> Vec<WorldEventMsg> {
    let mut out = Vec::new();
    for ev in world.block_broken.drain(..) {
        out.push(WorldEventMsg::BlockBroken {
            pos: ev.pos,
            block_id: ev.block.0,
            normal: ev.normal,
            tint: ev.tint,
        });
    }
    for (pos, block) in world.block_placed.drain(..) {
        out.push(WorldEventMsg::BlockPlaced {
            pos,
            block_id: block.0,
        });
    }
    for (anchor, open) in world.panel_changed.drain(..) {
        out.push(WorldEventMsg::PanelToggled { anchor, open });
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
    for fired in world.emitter_bursts.drain(..) {
        use crate::events::tick::BurstTexture;
        use crate::net::protocol::BurstTextureMsg;
        out.push(WorldEventMsg::EmitterBurst {
            emitter_id: fired.emitter,
            pos: fired.pos,
            intensity: fired.intensity,
            direction: fired.direction,
            texture: fired.texture.map(|texture| match texture {
                BurstTexture::Tile { slice, tint } => BurstTextureMsg::Tile {
                    tile: slice.tile.name().to_owned(),
                    slice: slice.slice,
                    tint,
                },
                BurstTexture::Block { block, tint } => BurstTextureMsg::Block {
                    block_id: block.0,
                    tint,
                },
            }),
        });
    }
    for s in world.sounds.drain(..) {
        out.push(WorldEventMsg::Sound {
            sound_id: s.sound.0,
            pos: s.pos,
        });
    }
    for c in world.spatial_sounds.drain(..) {
        use crate::events::tick::SpatialSoundCommand as Cmd;
        out.push(WorldEventMsg::SpatialSound(match c {
            Cmd::PlayAt {
                handle,
                sound,
                pos,
                volume,
                pitch,
            } => SpatialSoundMsg::PlayAt {
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
            } => SpatialSoundMsg::PlayOnMob {
                handle,
                sound_id: sound.0,
                mob_id,
                volume,
                pitch,
                last_pos,
            },
            Cmd::Set {
                handle,
                volume,
                pitch,
            } => SpatialSoundMsg::Set {
                handle,
                volume,
                pitch,
            },
            Cmd::Stop { handle } => SpatialSoundMsg::Stop { handle },
        }));
    }
    out
}

/// A body's replicated condition stages: `(condition id, stage)` in id order.
pub(super) fn condition_stages(
    conditions: &petramond_world::condition::BodyConditions,
) -> Vec<(u8, u8)> {
    conditions
        .active()
        .iter()
        .map(|c| (c.condition.0, c.stage()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::Broadcast;

    /// The environment ships in full on the first window, then only when it
    /// changed — and a joining session's reseed forces the next window to
    /// carry it again even though nothing changed.
    #[test]
    fn env_ships_on_change_and_after_a_reseed() {
        let mut broadcast = Broadcast::default();
        let params = std::sync::Arc::new(crate::world::environment::ShaderParamMap::from([(
            "petramond:sky".to_owned(),
            [1.0, 0.5, 0.25, 1.0],
        )]));
        assert!(broadcast.env_update(params.clone()).is_some(), "first window");
        assert!(broadcast.env_update(params.clone()).is_none(), "unchanged");
        broadcast.reseed_env();
        assert_eq!(
            broadcast.env_update(params),
            Some(vec![("petramond:sky".to_owned(), [1.0, 0.5, 0.25, 1.0])]),
            "a newcomer's window carries the full map"
        );
    }
}
