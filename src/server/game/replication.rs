use std::sync::Arc;

use crate::events::tick::{TickEvents, WorldEvents};
use crate::net::protocol::{
    BlockDelta, BlockDrawDelta, CellKvDelta, ClientEventMsg, ItemSlotWire, OpenScreen, SelfEvents,
    SelfState, SelfTransform, SleepTally, SpatialSoundMsg, TickUpdate, Transform, WorldEventMsg,
};
use crate::world::environment::ShaderParamMap;
use petramond_world::inventory::Hand;

use super::event_scope::{self, LiveLoop, Reach, Viewer};
use super::interest::view_blocks;
use super::{ServerGame, SharedTickRows};
use crate::net::spatial_loops::LiveSpatialLoops;

#[derive(Default)]
pub struct Broadcast {
    pending_wire_events: Vec<WorldEventMsg>,
    live_spatial_loops: LiveSpatialLoops,
    last_shipped_env: Option<Arc<ShaderParamMap>>,
}

impl Broadcast {
    pub fn bank_wire_events(&mut self, events: impl IntoIterator<Item = WorldEventMsg>) {
        self.pending_wire_events.extend(events);
    }

    pub fn take_wire_events(&mut self) -> Vec<WorldEventMsg> {
        std::mem::take(&mut self.pending_wire_events)
    }

    pub fn reseed_env(&mut self) {
        self.last_shipped_env = None;
    }

    pub fn env_update(&mut self, params: Arc<ShaderParamMap>) -> Option<Vec<(String, [f32; 4])>> {
        let changed: Vec<(String, [f32; 4])> = match &self.last_shipped_env {
            Some(last) if Arc::ptr_eq(last, &params) || **last == *params => return None,
            Some(last) => params
                .iter()
                .filter(|(k, v)| last.get(*k) != Some(*v))
                .map(|(k, v)| (k.clone(), *v))
                .collect(),
            None => params.iter().map(|(k, v)| (k.clone(), *v)).collect(),
        };
        self.last_shipped_env = Some(params);
        (!changed.is_empty()).then_some(changed)
    }

    pub fn spatial_loops(&self) -> &LiveSpatialLoops {
        &self.live_spatial_loops
    }

    pub fn spatial_loops_mut(&mut self) -> &mut LiveSpatialLoops {
        &mut self.live_spatial_loops
    }
}

#[derive(Default)]
pub struct WindowFeeds {
    events: Vec<WorldEventMsg>,
    reaches: Vec<Reach>,
    deltas: Vec<BlockDelta>,
    kv_deltas: Vec<CellKvDelta>,
    draw_deltas: Vec<BlockDrawDelta>,
    loops: Vec<LiveLoop>,
}

impl SharedTickRows {
    pub(super) fn with_feeds(mut self, feeds: WindowFeeds) -> Self {
        self.feeds = feeds;
        self
    }
}

impl ServerGame {
    pub(super) fn take_window_feeds(&mut self, events: &mut TickEvents) -> WindowFeeds {
        let mut world_events = self.broadcast.take_wire_events();
        world_events.extend(wire_world_events(&mut events.world));
        self.broadcast
            .track_spatial_loops(self.world.mobs(), &mut world_events);
        let mobs = self.world.mobs().instances();
        let loops = event_scope::live_loops(self.broadcast.spatial_loops(), |id| {
            mobs.iter()
                .find(|m| m.id() == id && !m.is_dead())
                .map(|m| m.pos)
        });
        let reaches = world_events.iter().map(event_scope::reach).collect();
        WindowFeeds {
            events: world_events,
            reaches,
            deltas: self.world.take_block_deltas(),
            kv_deltas: self.world.take_cell_kv_deltas(),
            draw_deltas: self.world.take_block_draw_deltas(),
            loops,
        }
    }

    pub fn shared_tick_rows(&mut self, events: &TickEvents) -> SharedTickRows {
        let recipients = self.entity_lanes(events);
        let sleep_tally = SleepTally {
            sleeping: self
                .sessions
                .iter()
                .filter(|s| s.sim.sleep.is_some())
                .count() as u16,
            connected: self.sessions.len() as u16,
        };
        let open_chests = self.containers.open_chests();
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
            feeds: WindowFeeds::default(),
        }
    }

    pub fn build_tick_update(
        &mut self,
        s: usize,
        events: &TickEvents,
        shared: &SharedTickRows,
    ) -> TickUpdate {
        let entities = &shared.recipients[s];
        let feeds = &shared.feeds;
        let terrain = &self.sessions[s].transport.terrain;
        let mut block_deltas: Vec<BlockDelta> = feeds
            .deltas
            .iter()
            .filter(|d| terrain.covers(d.pos))
            .cloned()
            .collect();
        let cell_kv_deltas: Vec<CellKvDelta> = feeds
            .kv_deltas
            .iter()
            .filter(|d| terrain.covers(d.pos))
            .cloned()
            .collect();
        let block_draws: Vec<BlockDrawDelta> = feeds
            .draw_deltas
            .iter()
            .filter(|d| terrain.covers(d.pos))
            .cloned()
            .collect();
        for pos in std::mem::take(&mut self.sessions[s].replication.pending_corrective_cells) {
            if !self.sessions[s].transport.terrain.covers(pos)
                || block_deltas.iter().any(|d| d.pos == pos)
            {
                continue;
            }
            if let Some(d) = self.world.block_delta_at(pos) {
                block_deltas.push(d);
            }
        }
        let world_events = self.scoped_world_events(s, feeds);

        let mut update = TickUpdate::new(shared.tick, shared.clock);
        update.push_list(self.sessions[s].sim.creative.take_replies());
        update.push_list(self.sessions[s].sim.schematic.take_notices());
        update.push_list(block_deltas);
        update.push_list(block_draws);
        update.push_list(cell_kv_deltas);
        if !entities.mobs.is_empty() {
            update.push(entities.mobs.clone());
        }
        if !entities.items.is_empty() {
            update.push(entities.items.clone());
        }
        if !entities.players.is_empty() {
            update.push(entities.players.clone());
        }
        if !entities.player_actions.is_empty() {
            update.push(Arc::clone(&entities.player_actions));
        }
        update.push(shared.sleep_tally);
        update.push_list(std::mem::take(
            &mut self.sessions[s].replication.pending_action_outcomes,
        ));
        update.push(self.build_self_state(s));
        if let Some(sync) = self.build_menu_sync(s) {
            update.push(sync);
        }
        if let Some(env) = &shared.env {
            update.push(env.clone());
        }
        update.push_list(shared.open_chests.clone());
        update.push_list(world_events);
        let self_events = self.build_self_events(s, events);
        if self_events != SelfEvents::default() {
            update.push(self_events);
        }
        update
    }

    fn scoped_world_events(&mut self, s: usize, feeds: &WindowFeeds) -> Vec<WorldEventMsg> {
        let view_cap = self.world.data().render_dist;
        let sess = &mut self.sessions[s];
        let presented_places: rustc_hash::FxHashSet<_> =
            std::mem::take(&mut sess.replication.presented_places)
                .into_iter()
                .collect();
        let presented_breaks: rustc_hash::FxHashSet<_> =
            std::mem::take(&mut sess.replication.presented_breaks)
                .into_iter()
                .collect();
        let pos = sess.player.pos;
        let t = &mut sess.transport;
        let view = view_blocks(t.view_radius, view_cap);
        let holds_cell = |p| t.terrain.covers(p);
        let viewer = Viewer {
            pos,
            view_blocks: view,
            holds_cell: &holds_cell,
        };
        let (mut out, stops) =
            event_scope::sync_loops(&viewer, &mut t.interest.loops, &feeds.loops);
        out.extend(
            feeds
                .events
                .iter()
                .zip(&feeds.reaches)
                .filter(|&(ev, &reach)| {
                    viewer.perceives(reach)
                        && match ev {
                            WorldEventMsg::BlockPlaced { pos, .. } => {
                                !presented_places.contains(pos)
                            }
                            WorldEventMsg::BlockBroken { pos, .. } => {
                                !presented_breaks.contains(pos)
                            }
                            _ => true,
                        }
                })
                .map(|(ev, _)| ev.clone()),
        );
        out.extend(stops);
        out
    }

    fn build_self_events(&mut self, s: usize, events: &TickEvents) -> SelfEvents {
        let p = events.player_at(s);
        let sess = &mut self.sessions[s];
        let id = sess.id;
        let gui = sess.replication.request_open_gui.take();
        let sleep = std::mem::take(&mut sess.replication.request_open_sleep);
        let open_screen = if let Some((kind, anchor)) = gui {
            petramond_world::gui_state::kind_key(kind).map(|kind_key| OpenScreen::Gui {
                kind_key: kind_key.to_string(),
                anchor,
            })
        } else if sleep {
            Some(OpenScreen::Sleep)
        } else {
            None
        };
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
        let inventory =
            (sess.replication.last_sent_inventory_revision != Some(revision)).then(|| {
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
                .sim
                .eating
                .as_ref()
                .is_some_and(|eat| eat.hand == petramond_world::inventory::Hand::Off),
            sleeping,
            sleep_bed,
            move_scale: player
                .claims
                .replicated_attribute(mod_api::PlayerAttribute::MoveSpeed),
            fly_scale: player
                .claims
                .replicated_attribute(mod_api::PlayerAttribute::FlySpeed),
            denied_actions: player.claims.replicated_denied_actions(),
            transform,
        }
    }

    pub fn build_menu_sync(&mut self, s: usize) -> Option<crate::net::protocol::MenuSyncMsg> {
        use crate::net::protocol::{GuiValueWire, MenuTargetWire};

        let base = self.build_menu_sync_base(s);
        let target = self.sessions[s].sim.menu.target();
        let gui_arc = target
            .kind()
            .is_some_and(|kind| kind.is_registered())
            .then(|| self.gui_state_for_menu_sync(s, target));
        let sess = &mut self.sessions[s];
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

    #[test]
    fn env_ships_on_change_and_after_a_reseed() {
        let mut broadcast = Broadcast::default();
        let params = std::sync::Arc::new(crate::world::environment::ShaderParamMap::from([(
            "petramond:sky".to_owned(),
            [1.0, 0.5, 0.25, 1.0],
        )]));
        assert!(
            broadcast.env_update(params.clone()).is_some(),
            "first window"
        );
        assert!(broadcast.env_update(params.clone()).is_none(), "unchanged");
        broadcast.reseed_env();
        assert_eq!(
            broadcast.env_update(params),
            Some(vec![("petramond:sky".to_owned(), [1.0, 0.5, 0.25, 1.0])]),
            "a newcomer's window carries the full map"
        );
    }

    #[test]
    fn env_ships_only_the_changed_params() {
        let mut broadcast = Broadcast::default();
        let map = |time: f32| {
            std::sync::Arc::new(crate::world::environment::ShaderParamMap::from([
                ("petramond:sky".to_owned(), [1.0, 0.5, 0.25, 1.0]),
                ("petramond:time".to_owned(), [time, 0.0, 0.0, 0.0]),
            ]))
        };
        assert_eq!(
            broadcast.env_update(map(0.1)).map(|rows| rows.len()),
            Some(2)
        );
        assert_eq!(
            broadcast.env_update(map(0.2)),
            Some(vec![("petramond:time".to_owned(), [0.2, 0.0, 0.0, 0.0])]),
            "the static sky param does not ride again"
        );
    }
}
