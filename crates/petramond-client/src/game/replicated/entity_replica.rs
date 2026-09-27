use std::collections::{HashMap, VecDeque};

use petramond::net::protocol::{PlayerMount, SleepTally};
use petramond::player::PlayerId;

use super::{ReplicatedItems, ReplicatedMobs, StagedRows, MAX_STAGED_ROW_BATCHES};
use crate::game::body_pose::MovementMedium;
use crate::game::remote_players::RemotePlayers;
use crate::game::tick::ReplicaClock;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Committed {
    pub dismounted: bool,
}

pub struct EntityReplica {
    mobs: ReplicatedMobs,
    items: ReplicatedItems,
    players: RemotePlayers,
    own_mount: Option<PlayerMount>,
    clock: ReplicaClock,
    staged: VecDeque<StagedRows>,
    committed_tick: u64,
    tick: u64,
    sleep_tally: SleepTally,
    self_id: PlayerId,
    roster: HashMap<PlayerId, String>,
}

impl EntityReplica {
    pub fn new(self_id: PlayerId, roster: impl IntoIterator<Item = (PlayerId, String)>) -> Self {
        Self {
            mobs: Default::default(),
            items: Default::default(),
            players: Default::default(),
            own_mount: None,
            clock: Default::default(),
            staged: VecDeque::new(),
            committed_tick: 0,
            tick: 0,
            sleep_tally: Default::default(),
            self_id,
            roster: roster.into_iter().collect(),
        }
    }

    #[inline]
    pub fn self_id(&self) -> PlayerId {
        self.self_id
    }

    #[inline]
    pub fn tick(&self) -> u64 {
        self.tick
    }

    #[inline]
    pub fn sleep_tally(&self) -> SleepTally {
        self.sleep_tally
    }

    pub fn roster(&self) -> &HashMap<PlayerId, String> {
        &self.roster
    }

    pub fn mobs(&self) -> &ReplicatedMobs {
        &self.mobs
    }

    pub fn items(&self) -> &ReplicatedItems {
        &self.items
    }

    pub fn players(&self) -> &RemotePlayers {
        &self.players
    }

    #[inline]
    pub fn own_mount(&self) -> Option<PlayerMount> {
        self.own_mount
    }

    #[inline]
    pub fn alpha(&self) -> f32 {
        self.clock.alpha()
    }

    #[inline]
    pub fn committed_tick(&self) -> u64 {
        self.committed_tick
    }

    pub fn advance_clock(&mut self, dt: f32) {
        self.clock.advance(dt);
    }

    pub fn player_joined(&mut self, id: PlayerId, name: String) {
        self.roster.insert(id, name);
    }

    pub fn player_left(&mut self, id: PlayerId) {
        self.roster.remove(&id);
    }

    pub fn set_tick(&mut self, tick: u64) {
        self.tick = tick;
    }

    pub fn receive(&mut self, sleep_tally: SleepTally, staged: StagedRows) -> Committed {
        self.sleep_tally = sleep_tally;
        if !self.clock.started() {
            debug_assert!(self.staged.is_empty());
            let committed = self.commit(staged);
            self.clock.start();
            committed
        } else {
            self.stage(staged);
            Committed::default()
        }
    }

    pub fn commit_next_due(&mut self) -> Option<Committed> {
        let due = if self.clock.overdue() {
            self.staged.pop_front()
        } else {
            None
        };
        let Some(staged) = due else {
            if self.staged.is_empty() {
                self.clock.hold();
            }
            return None;
        };
        let committed = self.commit(staged);
        self.clock.consume_segment();
        Some(committed)
    }

    pub fn commit_presented(&mut self, at: f64) -> Option<Committed> {
        let through = at.floor() as u64 + 1;
        let due = self
            .staged
            .front()
            .is_some_and(|rows| rows.windows().last().is_some_and(|w| w.tick <= through));
        if due {
            let staged = self.staged.pop_front().expect("checked above");
            return Some(self.commit(staged));
        }
        if self.clock.started() {
            self.clock.place((at - at.floor()) as f32);
        }
        None
    }

    pub fn advance_animation(
        &mut self,
        dt: f32,
        medium: impl Fn(petramond_math::world_pos::WorldPos) -> MovementMedium,
    ) {
        self.mobs.advance_anim_blends(dt);
        let alpha = self.clock.alpha();
        self.players.advance(dt, alpha, medium);
    }

    fn commit(&mut self, staged: StagedRows) -> Committed {
        let was_mounted = self.own_mount.is_some();
        for window in staged.windows() {
            self.committed_tick = window.tick;
            if let Some(own) = window.players.iter().find(|row| row.id == self.self_id) {
                self.own_mount = own.mount;
            }
            if staged.resync {
                self.mobs.resync(&window.mobs);
                self.items.resync(&window.items);
            } else {
                self.mobs.apply(&window.mobs);
                self.items.apply(&window.items);
            }
            self.players
                .apply(&window.players, self.self_id, staged.resync);
        }
        for window in staged.windows() {
            self.players.queue_actions(&window.actions);
        }
        Committed {
            dismounted: was_mounted && self.own_mount.is_none(),
        }
    }

    fn stage(&mut self, staged: StagedRows) {
        let staged = if self.staged.len() >= MAX_STAGED_ROW_BATCHES {
            let mut pending = self.staged.drain(..).chain(std::iter::once(staged));
            let mut folded = pending.next().expect("the queue was full");
            for rows in pending {
                folded.folded.push(rows.window);
                folded.folded.extend(rows.folded);
            }
            folded.resync = true;
            folded
        } else {
            staged
        };
        self.staged.push_back(staged);
        debug_assert!(self.staged.len() <= MAX_STAGED_ROW_BATCHES);
    }

    #[cfg(test)]
    pub fn mobs_mut(&mut self) -> &mut ReplicatedMobs {
        &mut self.mobs
    }

    #[cfg(test)]
    pub fn players_mut(&mut self) -> &mut RemotePlayers {
        &mut self.players
    }

    #[cfg(test)]
    pub fn clock_mut(&mut self) -> &mut ReplicaClock {
        &mut self.clock
    }

    #[cfg(test)]
    pub fn staged(&self) -> &VecDeque<StagedRows> {
        &self.staged
    }

    #[cfg(test)]
    pub fn set_own_mount(&mut self, mount: Option<PlayerMount>) {
        self.own_mount = mount;
    }
}
