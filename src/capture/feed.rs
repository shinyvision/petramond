use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use petramond_math::math::IVec3;

use super::body::CapturedActivity;
use super::source::{FileRanges, SourceFile};
use super::unpack::{Frame, FrameItem, StateBody};
use crate::net::protocol::{
    EntityLane, EntityRow, ItemLane, ItemStateRow, MobLane, MobStateRow, PlayerLane,
    PlayerStateRow, SelfState, ServerToClient, SleepTally, SpatialSoundMsg, TickUpdate,
    WorldEventMsg,
};
use crate::net::spatial_loops::{is_looped, loop_restarts, note_loop_commands, LiveSpatialLoops};
use crate::player::PlayerId;

#[derive(Clone, Debug, PartialEq)]
pub struct BatchMoment {
    pub tick: u64,
    pub clock: u64,
    pub mobs: Arc<[MobStateRow]>,
    pub items: Arc<[ItemStateRow]>,
    pub players: Arc<[PlayerStateRow]>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Moment {
    pub clock: Option<(u64, u64)>,
    pub mobs: BTreeMap<u64, MobStateRow>,
    pub items: BTreeMap<u64, ItemStateRow>,
    pub players: BTreeMap<PlayerId, PlayerStateRow>,
    pub roster: BTreeMap<PlayerId, String>,
    pub env: BTreeMap<String, [f32; 4]>,
    pub loops: LiveSpatialLoops,
    pub dig: Option<IVec3>,
    pub open_chests: Vec<IVec3>,
    pub viewer: Option<SelfState>,
    pub local_player: Option<PlayerId>,
    pub sleep: Option<SleepTally>,
    pub previous: Option<BatchMoment>,
}

impl Moment {
    pub fn restate(&mut self, body: &StateBody) {
        match body {
            StateBody::Clock(c) => self.clock = Some((c.tick, c.day_clock)),
            StateBody::Mob(row) => {
                self.mobs.insert(row.id, row.clone());
            }
            StateBody::Item(row) => {
                self.items.insert(row.id, row.clone());
            }
            StateBody::Player(row) => {
                self.players.insert(row.id, row.clone());
            }
            StateBody::Roster(entries) => {
                self.roster = entries
                    .iter()
                    .map(|e| (PlayerId(e.player.0), e.name.clone()))
                    .collect();
            }
            StateBody::Environment(env) => {
                self.env = env.params.iter().cloned().collect();
            }
            StateBody::Activity(CapturedActivity {
                loops,
                dig,
                open_chests,
            }) => {
                self.loops = loops.iter().map(|l| (loop_handle(l), *l)).collect();
                self.dig = *dig;
                self.open_chests = open_chests.clone();
            }
            StateBody::Viewer(own) => self.viewer = Some(own.clone()),
            StateBody::Session(session) => {
                self.local_player = Some(PlayerId(session.local_player.0))
            }
            StateBody::Section(_)
            | StateBody::Column(_)
            | StateBody::Presence(_)
            | StateBody::Population(_)
            | StateBody::Tables(_) => {}
        }
    }

    pub fn restrict(
        &mut self,
        population: &mod_api::capture::ClientPopulation,
    ) -> Result<(), String> {
        if let Some(id) = population
            .mobs
            .iter()
            .find(|id| !self.mobs.contains_key(id))
        {
            return Err(format!(
                "the population lists mob {id}, which nothing states"
            ));
        }
        if let Some(id) = population
            .items
            .iter()
            .find(|id| !self.items.contains_key(id))
        {
            return Err(format!(
                "the population lists item {id}, which nothing states"
            ));
        }
        if let Some(id) = population
            .players
            .iter()
            .find(|id| !self.players.contains_key(&PlayerId(id.0)))
        {
            return Err(format!(
                "the population lists player {}, which nothing states",
                id.0
            ));
        }
        self.mobs.retain(|id, _| population.mobs.contains(id));
        self.items.retain(|id, _| population.items.contains(id));
        self.players
            .retain(|id, _| population.players.contains(&mod_api::PlayerId(id.0)));
        Ok(())
    }

    pub fn message(&mut self, msg: &ServerToClient) {
        match msg {
            ServerToClient::PlayerJoined { id, name } => {
                self.roster.insert(*id, name.clone());
            }
            ServerToClient::PlayerLeft { id } => {
                self.roster.remove(id);
            }
            _ => {}
        }
    }

    pub fn batch(&mut self, t: &TickUpdate) {
        if let Some((tick, clock)) = self.clock {
            self.previous = Some(BatchMoment {
                tick,
                clock,
                mobs: self.mobs.values().cloned().collect(),
                items: self.items.values().cloned().collect(),
                players: self.players.values().cloned().collect(),
            });
        }
        self.clock = Some((t.tick, t.clock));
        if let Some(lane) = t.mobs() {
            lane.apply_to(&mut self.mobs);
        }
        if let Some(lane) = t.items() {
            lane.apply_to(&mut self.items);
        }
        if let Some(lane) = t.players() {
            lane.apply_to(&mut self.players);
        }
        self.open_chests = t.open_chests().cloned().unwrap_or_default();
        if let Some(tally) = t.sleep_tally() {
            self.sleep = Some(*tally);
        }
        if let Some(env) = t.env() {
            self.env.extend(env.iter().cloned());
        }
        if let Some(events) = t.events() {
            note_loop_commands(&mut self.loops, events, is_looped);
        }
        if let Some(own) = t.self_state() {
            self.viewer = Some(own.clone().over(self.viewer.take()));
        }
    }

    pub fn frame(&mut self, frame: &Frame) {
        for item in &frame.items {
            match item {
                FrameItem::Message(msg) => self.message(msg),
                FrameItem::Batch(t) => self.batch(t),
                FrameItem::Cues(cues) => self.dig = cues.dig,
                FrameItem::Terrain(_) | FrameItem::View(_) => {}
            }
        }
    }

    pub fn restatement(&self) -> Vec<ServerToClient> {
        let mut out: Vec<ServerToClient> = self
            .roster
            .iter()
            .map(|(&id, name)| ServerToClient::PlayerJoined {
                id,
                name: name.clone(),
            })
            .collect();
        if let Some(prev) = &self.previous {
            out.push(ServerToClient::Tick(Box::new(
                TickUpdate::new(prev.tick, prev.clock)
                    .with(MobLane::from(prev.mobs.to_vec()))
                    .with(ItemLane::from(prev.items.to_vec()))
                    .with(PlayerLane::from(prev.players.to_vec())),
            )));
        }
        let previous = self.previous.as_ref();
        let (tick, clock) = self.clock.unwrap_or_default();
        let mut newest = TickUpdate::new(tick, clock)
            .with(lane_from(previous.map(|p| &p.mobs[..]), &self.mobs))
            .with(lane_from(previous.map(|p| &p.items[..]), &self.items))
            .with(lane_from(previous.map(|p| &p.players[..]), &self.players));
        if let Some(tally) = self.sleep {
            newest.push(tally);
        }
        if let Some(own) = &self.viewer {
            newest.push(own.clone());
        }
        newest.push_list(
            self.env
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect::<Vec<_>>(),
        );
        newest.push_list(self.open_chests.clone());
        newest.push_list(loop_restarts(&self.loops).collect::<Vec<WorldEventMsg>>());
        out.push(ServerToClient::Tick(Box::new(newest)));
        out
    }
}

fn lane_from<R: EntityRow>(before: Option<&[R]>, now: &BTreeMap<R::Id, R>) -> EntityLane<R, R::Id> {
    let mut lane = EntityLane::from(now.values().cloned().collect::<Vec<R>>());
    lane.despawned = before
        .unwrap_or_default()
        .iter()
        .map(EntityRow::entity_id)
        .filter(|id| !now.contains_key(id))
        .collect();
    lane.despawned.sort_unstable();
    lane
}

fn loop_handle(cmd: &SpatialSoundMsg) -> u64 {
    match *cmd {
        SpatialSoundMsg::PlayAt { handle, .. }
        | SpatialSoundMsg::PlayOnMob { handle, .. }
        | SpatialSoundMsg::Stop { handle }
        | SpatialSoundMsg::Set { handle, .. } => handle,
    }
}

#[derive(Clone, Debug, Default)]
pub struct Queue {
    ranges: VecDeque<Span>,
}

#[derive(Clone, Debug)]
pub struct Span {
    pub file: SourceFile,
    pub offset: u64,
    pub end: u64,
}

impl Queue {
    pub fn push(&mut self, ranges: &[FileRanges]) {
        for r in ranges {
            for &[offset, len] in &r.ranges {
                if len > 0 {
                    self.ranges.push_back(Span {
                        file: r.file.clone(),
                        offset,
                        end: offset + len,
                    });
                }
            }
        }
    }

    pub fn front(&self) -> Option<&Span> {
        self.ranges.front()
    }

    pub fn spans(&self) -> impl Iterator<Item = &Span> {
        self.ranges.iter()
    }

    pub fn advance(&mut self, len: u64) {
        if let Some(span) = self.ranges.front_mut() {
            span.offset += len;
            if span.offset >= span.end {
                self.ranges.pop_front();
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    pub fn clear(&mut self) {
        self.ranges.clear();
    }

    pub fn forget(&mut self, incarnation: u64) {
        self.ranges.retain(|s| s.file.incarnation != incarnation);
    }
}
