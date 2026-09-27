//! The replicated ENTITY state: mob / item / remote-player stores, the own
//! row's mount, the staged interpolation window over them, and the per-batch
//! session facts (tick, sleep headcount, roster).
//!
//! Entity rows never apply on arrival: each batch's rows are STAGED and
//! committed only when render time crosses into their segment (see
//! [`ReplicaClock`]), so the committed prev→curr pair under the render never
//! shifts mid-segment. This type is the only writer of those stores; every
//! reader goes through its accessors.

use std::collections::{HashMap, VecDeque};

use petramond::net::protocol::{PlayerMount, SleepTally};
use petramond::player::PlayerId;

use super::{ReplicatedItems, ReplicatedMobs, StagedRows, MAX_STAGED_ROW_BATCHES};
use crate::game::body_pose::MovementMedium;
use crate::game::remote_players::RemotePlayers;
use crate::game::tick::ReplicaClock;

/// What committing one staged batch changed that the local player must act
/// on.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Committed {
    /// The own row's mount went from `Some` to `None`: the local body must
    /// land beside the hull the way the server's riding pass did.
    pub dismounted: bool,
}

pub struct EntityReplica {
    /// REPLICATED mob store: presentation reads these, never the server's.
    mobs: ReplicatedMobs,
    /// REPLICATED dropped-item store (same contract as `mobs`).
    items: ReplicatedItems,
    /// Every OTHER session in this client's interest — its prev/curr row pair
    /// plus its body-pose / held-item animation state. The local player is
    /// never in it.
    players: RemotePlayers,
    /// The authoritative mount from the own replicated player row —
    /// `(stable mob id, seat index)`. While `Some`, local player physics and
    /// entity push are suspended and the body slaves to the interpolated
    /// mount.
    own_mount: Option<PlayerMount>,
    /// Client-side tick clock over RECEIVED batches — the `tick_alpha`
    /// source.
    clock: ReplicaClock,
    /// Bounded FIFO of batches waiting for crossed render-time segment
    /// boundaries.
    staged: VecDeque<StagedRows>,
    /// The tick of the rows the interpolation window closes on.
    committed_tick: u64,
    /// The latest replicated tick number — the client's notion of game time
    /// for presentation scheduling.
    tick: u64,
    /// The server-wide sleep headcount from the latest batch — remote rows
    /// only cover the players in view, the overlay counts everyone.
    sleep_tally: SleepTally,
    /// The LOCAL player's server-assigned id — distinguishes own vs foreign
    /// rows and events.
    self_id: PlayerId,
    /// The OTHER connected players (id → name): seeded from the join, then
    /// maintained by `PlayerJoined`/`PlayerLeft` broadcasts.
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

    /// The interpolation fraction between the committed prev→curr rows.
    #[inline]
    pub fn alpha(&self) -> f32 {
        self.clock.alpha()
    }

    /// The tick of the rows the interpolation window closes on.
    #[inline]
    pub fn committed_tick(&self) -> u64 {
        self.committed_tick
    }

    /// Advance render time by one frame's `dt`.
    pub fn advance_clock(&mut self, dt: f32) {
        self.clock.advance(dt);
    }

    pub fn player_joined(&mut self, id: PlayerId, name: String) {
        self.roster.insert(id, name);
    }

    pub fn player_left(&mut self, id: PlayerId) {
        self.roster.remove(&id);
    }

    /// Record a batch's tick number.
    pub fn set_tick(&mut self, tick: u64) {
        self.tick = tick;
    }

    /// Adopt one batch's entity rows and sleep headcount. The first batch
    /// renders directly (prev == curr — there is nothing to interpolate
    /// from) and starts the timeline; every later one is staged.
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

    /// Turn the interpolation window by one crossed segment: commit the
    /// oldest staged batch when render time is overdue. `None` once nothing
    /// more is due — a starved queue then holds at the segment end. Outside
    /// the first batch, this is the ONLY path that shifts committed rows.
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

    /// A presentation's window: commit the next staged batch its position
    /// `at` (in ticks) has reached, the pair around `at` being the one to
    /// close on; once nothing more is due, place render time at `at`'s
    /// fraction. The presentation's position is the clock — it never runs
    /// one of its own.
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

    /// One frame of entity animation after the batches applied: named-mob
    /// blend weights ease toward their committed targets, and remote bodies
    /// advance their pose / hand / hurt state at this frame's alpha.
    pub fn advance_animation(
        &mut self,
        dt: f32,
        medium: impl Fn(petramond_math::world_pos::WorldPos) -> MovementMedium,
    ) {
        self.mobs.advance_anim_blends(dt);
        let alpha = self.clock.alpha();
        self.players.advance(dt, alpha, medium);
    }

    /// Adopt entity rows into the committed stores. Ordinary batches shift
    /// curr→prev; an overflow resync seeds prev == curr for every entity so a
    /// dropped backlog cannot become one segment of extreme-speed motion. The
    /// own row's mount adopts HERE — the local body slaves to the same
    /// committed pair every observer renders.
    fn commit(&mut self, staged: StagedRows) -> Committed {
        let was_mounted = self.own_mount.is_some();
        // A folded entry replays its windows oldest first, so every entity
        // ends on its newest row; a resync's rows seed the pair in place
        // rather than interpolate across the dropped gap.
        for window in staged.windows() {
            self.committed_tick = window.tick;
            // The own row always rides (a session tracks itself); a window
            // that leaves it out left it unchanged.
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
        // Actions land after every window's rows, so a body that entered
        // interest anywhere in a folded span still takes its earlier edges.
        for window in staged.windows() {
            self.players.queue_actions(&window.actions);
        }
        Committed {
            dismounted: was_mounted && self.own_mount.is_none(),
        }
    }

    /// Queue one post-bootstrap batch. Overflow is a declared resync: every
    /// pending window folds into one entry that keeps each window's shared
    /// lanes and actions (lanes compose in order, so no spawn or despawn is
    /// lost and each entity ends on its newest row; every dropped batch's
    /// player actions survive in arrival order so one-shot animation triggers
    /// are not lost). No row is copied: the fold moves windows, and the
    /// commit replays them.
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

    /// Test access to the stores and the window, for tests that drive them
    /// with hand-built rows.
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
