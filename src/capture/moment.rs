//! The MOMENT: every part of the presented world that is not terrain — the
//! clock, the newest batch's rows, the roster, the environment, what sounds
//! and stands open, the local player's own state, and the cells it predicts.
//!
//! The client session holds each part as an `Arc` and swaps one only when it
//! changes, stamping its key in the world's revisions as it does. Publishing
//! the moment is therefore a handful of refcount bumps, and a capture takes
//! it the same way.

use std::sync::Arc;

use mod_api::capture::{
    ClientCapturedEnv, ClientCapturedSession, ClientPopulation, ClientRosterEntry, ClientStateKey,
};
use rustc_hash::FxHashMap;

use super::body::CapturedActivity;
use crate::net::protocol::{ItemStateRow, MobStateRow, PlayerStateRow, SelfState};
use crate::world::Changes;
use petramond_math::math::IVec3;

/// The newest applied batch's rows: every entity present.
#[derive(Clone, Default)]
pub struct Rows {
    pub mobs: Arc<[MobStateRow]>,
    pub items: Arc<[ItemStateRow]>,
    pub players: Arc<[PlayerStateRow]>,
}

impl Rows {
    pub fn population(&self) -> ClientPopulation {
        ClientPopulation {
            mobs: self.mobs.iter().map(|r| r.id).collect(),
            items: self.items.iter().map(|r| r.id).collect(),
            players: self
                .players
                .iter()
                .map(|r| mod_api::PlayerId(r.id.0))
                .collect(),
        }
    }
}

#[derive(Clone)]
pub struct Moment {
    pub session: Arc<ClientCapturedSession>,
    /// The newest applied batch's tick.
    pub tick: u64,
    pub day_clock: u64,
    pub rows: Rows,
    pub roster: Arc<[ClientRosterEntry]>,
    pub env: Arc<ClientCapturedEnv>,
    pub activity: Arc<CapturedActivity>,
    /// The local player's own state as the authority last stated it.
    pub viewer: Option<Arc<SelfState>>,
    /// Cells holding an unconfirmed local prediction.
    pub predicted: Arc<[IVec3]>,
    /// The fractional tick the last frame presented.
    pub presented_tick: f64,
}

impl Moment {
    pub fn new(session: ClientCapturedSession) -> Self {
        Self {
            session: Arc::new(session),
            tick: 0,
            day_clock: 0,
            rows: Rows::default(),
            roster: Arc::from(Vec::new()),
            env: Arc::new(ClientCapturedEnv { params: Vec::new() }),
            activity: Arc::default(),
            viewer: None,
            predicted: Arc::from(Vec::new()),
            presented_tick: 0.0,
        }
    }

    /// Whether `key`, a moment key, is present.
    pub fn holds(&self, key: ClientStateKey) -> bool {
        match key {
            ClientStateKey::Mob(id) => self.rows.mobs.iter().any(|r| r.id == id),
            ClientStateKey::Item(id) => self.rows.items.iter().any(|r| r.id == id),
            ClientStateKey::Player(id) => self.rows.players.iter().any(|r| r.id.0 == id.0),
            ClientStateKey::Viewer => self.viewer.is_some(),
            ClientStateKey::Section(_) | ClientStateKey::Column(_) | ClientStateKey::Presence => {
                false
            }
            _ => true,
        }
    }

    /// Adopt a newer batch's rows, stamping exactly the entities whose row
    /// differs from the one before, and `Population` when the set moved.
    pub fn set_rows(&mut self, rows: Rows, changes: &mut Changes) {
        stamp_rows(
            &self.rows.mobs,
            &rows.mobs,
            |r| r.id,
            ClientStateKey::Mob,
            changes,
        );
        stamp_rows(
            &self.rows.items,
            &rows.items,
            |r| r.id,
            ClientStateKey::Item,
            changes,
        );
        stamp_rows(
            &self.rows.players,
            &rows.players,
            |r| u64::from(r.id.0),
            |id| ClientStateKey::Player(mod_api::PlayerId(id as u8)),
            changes,
        );
        self.rows = rows;
    }

    /// The newest batch's tick and the day clock.
    pub fn set_clock(&mut self, tick: u64, day_clock: u64, changes: &mut Changes) {
        if (tick, day_clock) != (self.tick, self.day_clock) {
            self.tick = tick;
            self.day_clock = day_clock;
            changes.stamp(ClientStateKey::Clock);
        }
    }

    pub fn set_roster(&mut self, roster: Vec<ClientRosterEntry>, changes: &mut Changes) {
        if *self.roster != *roster {
            self.roster = roster.into();
            changes.stamp(ClientStateKey::Roster);
        }
    }

    pub fn set_env(&mut self, env: ClientCapturedEnv, changes: &mut Changes) {
        if *self.env != env {
            self.env = Arc::new(env);
            changes.stamp(ClientStateKey::Environment);
        }
    }

    pub fn set_activity(&mut self, activity: CapturedActivity, changes: &mut Changes) {
        if *self.activity != activity {
            self.activity = Arc::new(activity);
            changes.stamp(ClientStateKey::Activity);
        }
    }

    pub fn set_viewer(&mut self, viewer: Option<SelfState>, changes: &mut Changes) {
        if self.viewer.as_deref() != viewer.as_ref() {
            match viewer {
                Some(state) => {
                    self.viewer = Some(Arc::new(state));
                    changes.stamp(ClientStateKey::Viewer);
                }
                None => {
                    self.viewer = None;
                    changes.remove(ClientStateKey::Viewer);
                }
            }
        }
    }
}

fn stamp_rows<R: PartialEq>(
    old: &[R],
    new: &[R],
    id: impl Fn(&R) -> u64,
    key: impl Fn(u64) -> ClientStateKey,
    changes: &mut Changes,
) {
    if std::ptr::eq(old, new) {
        return;
    }
    let before: FxHashMap<u64, &R> = old.iter().map(|r| (id(r), r)).collect();
    let mut moved = false;
    let mut kept = 0usize;
    for row in new {
        let row_id = id(row);
        match before.get(&row_id) {
            Some(prev) => {
                kept += 1;
                if *prev != row {
                    changes.stamp(key(row_id));
                }
            }
            None => {
                moved = true;
                changes.stamp(key(row_id));
            }
        }
    }
    if kept < before.len() {
        moved = true;
        let now: rustc_hash::FxHashSet<u64> = new.iter().map(&id).collect();
        for gone in before.keys().filter(|k| !now.contains(k)) {
            changes.remove(key(*gone));
        }
    }
    if moved {
        changes.stamp(ClientStateKey::Population);
    }
}
