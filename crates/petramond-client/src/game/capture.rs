use std::collections::BTreeMap;
use std::sync::Arc;

use mod_api::capture::{
    ClientCapturedEnv, ClientCapturedMod, ClientCapturedSession, ClientCapturedView,
    ClientRosterEntry,
};
use petramond::capture::body::{CapturedActivity, CapturedCues};
use petramond::capture::events::{Applied, FrameInput};
use petramond::capture::moment::{Moment, Rows};
use petramond::capture::state::{ColumnSnap, SectionSnap};
use petramond::capture::view::ViewCue;
use petramond::net::protocol::{
    ItemStateRow, MobStateRow, PlayerStateRow, ServerToClient, TickUpdate, WorldEventMsg,
};
use petramond::net::spatial_loops::{is_looped, note_loop_commands, LiveSpatialLoops};
use petramond::player::PlayerId;
use petramond_math::math::IVec3;

use super::tick::WorldEvent;
use super::Game;

#[derive(Clone, Debug, Default)]
pub struct SessionIdentity {
    pub player_name: String,
    pub mods: Vec<petramond::modding::modset::ModSetEntry>,
}

pub(super) struct WorldCapture {
    identity: SessionIdentity,
    moment: Option<Moment>,
    logging: bool,
    drained: Vec<ServerToClient>,
    applied: Vec<Applied>,
    cues: Option<CapturedCues>,
    dig: Option<IVec3>,
    loops: LiveSpatialLoops,
    tracked: Tracked,
    roster_dirty: bool,
    batch: Option<Batch>,
}

#[derive(Default)]
struct Tracked {
    mobs: BTreeMap<u64, MobStateRow>,
    items: BTreeMap<u64, ItemStateRow>,
    players: BTreeMap<PlayerId, PlayerStateRow>,
    rows: Rows,
}

impl Tracked {
    fn apply(&mut self, t: &TickUpdate) {
        if let Some(lane) = t.mobs().filter(|lane| !lane.is_empty()) {
            lane.apply_to(&mut self.mobs);
            self.rows.mobs = self.mobs.values().cloned().collect();
        }
        if let Some(lane) = t.items().filter(|lane| !lane.is_empty()) {
            lane.apply_to(&mut self.items);
            self.rows.items = self.items.values().cloned().collect();
        }
        if let Some(lane) = t.players().filter(|lane| !lane.is_empty()) {
            lane.apply_to(&mut self.players);
            self.rows.players = self.players.values().cloned().collect();
        }
    }
}

struct Batch {
    tick: u64,
    clock: u64,
    rows: Rows,
    own: bool,
    env: bool,
}

impl Game {
    pub(super) fn capture_player_name(&self) -> Option<&str> {
        Some(self.world_capture.identity.player_name.as_str()).filter(|n| !n.is_empty())
    }

    pub(super) fn capture_entities_reset(&mut self) {
        self.world_capture.tracked = Tracked::default();
    }
}

impl WorldCapture {
    pub(super) fn new(identity: &SessionIdentity) -> Self {
        Self {
            identity: identity.clone(),
            moment: None,
            logging: false,
            drained: Vec::new(),
            applied: Vec::new(),
            cues: None,
            dig: None,
            loops: LiveSpatialLoops::new(),
            tracked: Tracked::default(),
            roster_dirty: true,
            batch: None,
        }
    }
}

fn world_only(msg: &ServerToClient) -> Option<ServerToClient> {
    match msg {
        ServerToClient::ColumnData(_)
        | ServerToClient::SectionData(_)
        | ServerToClient::SectionCached { .. }
        | ServerToClient::LightData(_)
        | ServerToClient::PlayerJoined { .. }
        | ServerToClient::PlayerLeft { .. }
        | ServerToClient::Tick(_) => Some(msg.clone()),
        ServerToClient::SectionUnload { pos, .. } => Some(ServerToClient::SectionUnload {
            pos: *pos,
            cache_hash: None,
        }),
        ServerToClient::ColumnUnload { pos, .. } => Some(ServerToClient::ColumnUnload {
            pos: *pos,
            cache_hashes: Vec::new(),
        }),
        ServerToClient::HelloAck { .. }
        | ServerToClient::HelloReject { .. }
        | ServerToClient::ModList { .. }
        | ServerToClient::JoinAccept(_)
        | ServerToClient::JoinReject { .. }
        | ServerToClient::StreamBatchStart
        | ServerToClient::StreamBatchEnd { .. }
        | ServerToClient::ChatLine(_)
        | ServerToClient::RecipesUnlocked { .. }
        | ServerToClient::ModsDisabled { .. }
        | ServerToClient::ServerClosing
        | ServerToClient::KeepAlive
        | ServerToClient::Disconnect { .. } => None,
    }
}

impl Game {
    pub(super) fn capture_drained(&mut self, msgs: &[ServerToClient]) {
        let tee = &mut self.world_capture;
        for msg in msgs {
            match msg {
                ServerToClient::Tick(t) => {
                    if let Some(events) = t.events() {
                        note_loop_commands(&mut tee.loops, events, is_looped);
                    }
                    tee.tracked.apply(t);
                    tee.batch = Some(Batch {
                        tick: t.tick,
                        clock: t.clock,
                        rows: tee.tracked.rows.clone(),
                        own: t.self_state().is_some() || tee.batch.as_ref().is_some_and(|b| b.own),
                        env: t.env().is_some() || tee.batch.as_ref().is_some_and(|b| b.env),
                    });
                }
                ServerToClient::PlayerJoined { .. } | ServerToClient::PlayerLeft { .. } => {
                    tee.roster_dirty = true;
                }
                _ => {}
            }
            if tee.logging {
                tee.drained.extend(world_only(msg));
            }
        }
    }

    pub(super) fn capture_applied(&mut self) {
        let batch = self.world_capture.batch.take();
        if batch.is_some() || self.world_capture.moment.is_none() || self.world_capture.roster_dirty
        {
            self.update_moment(batch);
        }
        let drained = std::mem::take(&mut self.world_capture.drained);
        for msg in drained {
            let entry = match msg {
                ServerToClient::SectionData(p) => {
                    SectionSnap::of(&self.replica.world, p.pos).map(Applied::Section)
                }
                ServerToClient::SectionCached { pos, .. } => {
                    SectionSnap::of(&self.replica.world, pos).map(Applied::Section)
                }
                ServerToClient::ColumnData(p) => {
                    ColumnSnap::of(&self.replica.world, p.pos).map(Applied::Column)
                }
                ServerToClient::Tick(t) => Some(Applied::Batch(t)),
                other => Some(Applied::Message(other)),
            };
            self.world_capture.applied.extend(entry);
        }
    }

    pub(super) fn present_predicted_world_event(&mut self, event: WorldEvent) {
        if self.world_capture.logging {
            let wire = match event {
                WorldEvent::BlockBroken {
                    pos,
                    block,
                    normal,
                    tint,
                } => Some(WorldEventMsg::BlockBroken {
                    pos,
                    block_id: block.id(),
                    normal,
                    tint,
                }),
                WorldEvent::BlockPlaced { pos, block } => Some(WorldEventMsg::BlockPlaced {
                    pos,
                    block_id: block.id(),
                }),
                _ => None,
            };
            if let Some(wire) = wire {
                let dig = self.world_capture.dig;
                self.world_capture
                    .cues
                    .get_or_insert_with(|| CapturedCues {
                        events: Vec::new(),
                        dig,
                    })
                    .events
                    .push(wire);
            }
        }
        self.replica.events.world.push(event);
    }

    pub(super) fn record_dig(&mut self, break_held: bool) {
        let cell = self
            .replica
            .self_view
            .mining
            .filter(|_| break_held)
            .map(|(cell, _)| cell);
        let tee = &mut self.world_capture;
        if cell == tee.dig {
            return;
        }
        tee.dig = cell;
        if tee.logging {
            tee.cues
                .get_or_insert_with(|| CapturedCues {
                    events: Vec::new(),
                    dig: cell,
                })
                .dig = cell;
        }
        self.update_moment(None);
    }

    pub fn capture_wants_view(&self) -> bool {
        self.world_capture.logging
    }

    pub(super) fn capture_frame_begin(&mut self) {
        let desk = self.client_mods.presented().lock().capture.clone();
        let logging = desk.lock().begin_frame();
        self.world_capture.logging = logging;
        self.replica.world.changes_mut().set_collecting(logging);
    }

    pub fn capture_frame_end(&mut self, view: Option<ViewCue>) {
        self.capture_frame_begin();
        let changes = self.replica.world.changes_mut().end_frame();
        let presented_tick =
            self.replica.entities.committed_tick() as f64 - 1.0 + f64::from(self.tick_alpha());
        let desk = self.client_mods.presented().lock().capture.clone();
        let mut desk = desk.lock();
        if let Some(moment) = self.world_capture.moment.as_mut() {
            moment.presented_tick = presented_tick;
            desk.moment = Some(moment.clone());
        }
        for owner in desk.logs.owners() {
            if !self.client_mods.is_live(&owner) {
                desk.logs
                    .end_all_of(&owner, &format!("'{owner}', which began it, stopped"));
            }
        }
        let tee = &mut self.world_capture;
        let applied = std::mem::take(&mut tee.applied);
        let cues = tee.cues.take();
        if tee.logging {
            let input = self.capture_frame_input(presented_tick, applied, cues, view, changes);
            desk.submit_frame(input, &self.jobs);
        }
        desk.end_frame();
    }

    fn capture_frame_input(
        &self,
        presented_tick: f64,
        applied: Vec<Applied>,
        cues: Option<CapturedCues>,
        view: Option<ViewCue>,
        changes: petramond::world::FrameChanges,
    ) -> FrameInput {
        let player = mod_api::PlayerId(self.replica.entities.self_id().0);
        let view = view.map(|cue| {
            let pose = ClientCapturedView {
                player,
                pos: [cue.pos.x, cue.pos.y, cue.pos.z],
                yaw: cue.yaw,
                pitch: cue.pitch,
                roll: cue.roll,
                fov_y: cue.fov_y,
            };
            (cue, pose)
        });
        let predicted = self
            .world_capture
            .moment
            .as_ref()
            .map_or_else(|| Arc::from(Vec::new()), |m| Arc::clone(&m.predicted));
        FrameInput {
            world: self.replica.world.changes().base(),
            presented_tick,
            applied,
            cues,
            view,
            changes,
            predicted,
        }
    }

    fn capture_roster(&self) -> Vec<ClientRosterEntry> {
        let self_id = self.replica.entities.self_id();
        let mut roster: Vec<ClientRosterEntry> = self
            .replica
            .entities
            .roster()
            .iter()
            .map(|(id, name)| ClientRosterEntry {
                player: mod_api::PlayerId(id.0),
                name: name.clone(),
            })
            .collect();
        let admitted = &self.world_capture.identity.player_name;
        if !admitted.is_empty() && !roster.iter().any(|e| e.player.0 == self_id.0) {
            roster.push(ClientRosterEntry {
                player: mod_api::PlayerId(self_id.0),
                name: admitted.clone(),
            });
        }
        roster.sort_unstable_by_key(|e| e.player.0);
        roster
    }

    fn update_moment(&mut self, batch: Option<Batch>) {
        let session = || ClientCapturedSession {
            seed: self.replica.world.data().seed,
            local_player: mod_api::PlayerId(self.replica.entities.self_id().0),
            mods: self
                .world_capture
                .identity
                .mods
                .iter()
                .map(|m| ClientCapturedMod {
                    id: m.id.clone(),
                    version: m.version.clone(),
                    affects_world: m.affects_world,
                })
                .collect(),
        };
        let first = self.world_capture.moment.is_none();
        let mut moment = match self.world_capture.moment.take() {
            Some(moment) => moment,
            None => {
                let moment = Moment::new(session());
                let changes = self.replica.world.changes_mut();
                for key in [
                    mod_api::ClientStateKey::Clock,
                    mod_api::ClientStateKey::Population,
                    mod_api::ClientStateKey::Roster,
                    mod_api::ClientStateKey::Environment,
                    mod_api::ClientStateKey::Activity,
                ] {
                    changes.stamp(key);
                }
                moment
            }
        };
        let changes = self.replica.world.changes_mut();
        let env_changed = batch.as_ref().is_some_and(|b| b.env);
        if let Some(b) = batch {
            moment.set_clock(b.tick, b.clock, changes);
            moment.set_rows(b.rows, changes);
            if b.own {
                moment.set_viewer(Some(self.replica.self_view.to_wire()), changes);
            }
        }
        if std::mem::take(&mut self.world_capture.roster_dirty) {
            let roster = self.capture_roster();
            moment.set_roster(roster, self.replica.world.changes_mut());
        }
        if first || env_changed {
            let mut params: Vec<(String, [f32; 4])> = self
                .replica
                .world
                .data()
                .environment()
                .shader_params()
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect();
            params.sort_unstable_by(|a, b| a.0.cmp(&b.0));
            moment.set_env(
                ClientCapturedEnv { params },
                self.replica.world.changes_mut(),
            );
        }
        let changes = self.replica.world.changes_mut();
        let mut open_chests: Vec<IVec3> = self.fx.open_chests().iter().copied().collect();
        open_chests.sort_unstable_by_key(|c| (c.x, c.y, c.z));
        let activity = CapturedActivity {
            loops: self.world_capture.loops.values().copied().collect(),
            dig: self.world_capture.dig,
            open_chests,
        };
        moment.set_activity(activity, changes);
        let predicted: Vec<IVec3> = self.prediction.predicted_cells().collect();
        if *moment.predicted != *predicted {
            moment.predicted = predicted.into();
        }
        let desk = self.client_mods.presented().lock().capture.clone();
        desk.lock().moment = Some(moment.clone());
        self.world_capture.moment = Some(moment);
    }
}
