//! The registry of camp posts this session knows, and when an empty one may be filled.
//!
//! Posts are found per section, from the camp generator's cell marker, whenever a section
//! generates or loads; a rescanned section replaces what was known about it. A post is
//! occupied while a live skeleton names it. An empty post refills once its section has settled
//! (the guard saved with it restores alongside it) and, if its guard was killed, once the respawn
//! delay has passed. A guard that merely despawned or unloaded is not waited for. The first guard
//! a post ever gets may appear in view, like the camp itself arriving; a replacement may not.

use std::collections::BTreeMap;

use mod_sdk::MobTagValue;

use super::geometry::distance;
use crate::post_marker::{PostMarker, PostRole};

/// Ticks after a section (re)loads before its posts count as empty.
pub const SETTLE: u64 = 40;

/// Ticks a killed guard's post stays empty.
pub const RESPAWN: u64 = 20 * 60 * 5;

/// Ticks a section that was not ready yet is asked about again.
pub const RETRY_WINDOW: u64 = 200;

/// A post fills only while some player is at least this close...
pub const SPAWN_MAX: f64 = 96.0;
/// ...and none is closer than this...
pub const SPAWN_MIN: f64 = 20.0;
/// ...and no player within this distance can see it.
pub const SIGHT: f64 = 48.0;

/// A guard's role tag: its post's role.
pub fn role_tag(role: PostRole) -> MobTagValue {
    MobTagValue::I64(i64::from(role.code()))
}

/// The role a guard's role tag names.
pub fn tagged_role(value: &MobTagValue) -> Option<PostRole> {
    match value {
        MobTagValue::I64(code) => u8::try_from(*code).ok().and_then(PostRole::from_code),
        _ => None,
    }
}

/// Where a guard of the post at floor cell `cell` stands: feet centre on the floor's top.
pub fn standing_point(cell: [i32; 3]) -> [f64; 3] {
    [
        f64::from(cell[0]) + 0.5,
        f64::from(cell[1]) + 1.0,
        f64::from(cell[2]) + 0.5,
    ]
}

#[derive(Clone, Debug, PartialEq)]
pub struct Post {
    pub marker: PostMarker,
    section: [i32; 3],
    loaded_at: u64,
    died_at: Option<u64>,
    occupied: bool,
    /// Whether a guard has ever held this post since the registry learned of it.
    manned: bool,
}

/// What asking a section for its markers found.
pub enum Scan {
    /// The section's content is not final yet.
    NotReady,
    Found(Vec<([i32; 3], PostMarker)>),
}

#[derive(Default)]
pub struct Posts {
    posts: BTreeMap<[i32; 3], Post>,
    sections: BTreeMap<[i32; 3], Vec<[i32; 3]>>,
    pending: BTreeMap<[i32; 3], u64>,
}

impl Posts {
    pub fn queue(&mut self, section: [i32; 3], now: u64) {
        self.pending.insert(section, now);
    }

    pub fn pending(&self) -> Vec<[i32; 3]> {
        self.pending.keys().copied().collect()
    }

    pub fn get(&self, cell: &[i32; 3]) -> Option<&Post> {
        self.posts.get(cell)
    }

    pub fn settle(&mut self, section: [i32; 3], scan: Scan, now: u64) {
        let found = match scan {
            Scan::NotReady => {
                if self
                    .pending
                    .get(&section)
                    .is_some_and(|&queued| now.saturating_sub(queued) > RETRY_WINDOW)
                {
                    self.pending.remove(&section);
                }
                return;
            }
            Scan::Found(found) => found,
        };
        self.pending.remove(&section);
        for cell in self.sections.remove(&section).unwrap_or_default() {
            if !found.iter().any(|(c, _)| *c == cell) {
                self.posts.remove(&cell);
            }
        }
        if found.is_empty() {
            return;
        }
        let cells = found.iter().map(|(c, _)| *c).collect();
        for (cell, marker) in found {
            let post = self.posts.entry(cell).or_insert(Post {
                marker,
                section,
                loaded_at: now,
                died_at: None,
                occupied: false,
                manned: false,
            });
            post.marker = marker;
            post.section = section;
            post.loaded_at = now;
        }
        self.sections.insert(section, cells);
    }

    pub fn record_death(&mut self, cell: [i32; 3], now: u64) {
        if let Some(post) = self.posts.get_mut(&cell) {
            post.died_at = Some(now);
            post.occupied = false;
            post.manned = true;
        }
    }

    pub fn set_occupied(&mut self, cell: [i32; 3]) {
        if let Some(post) = self.posts.get_mut(&cell) {
            post.occupied = true;
            post.manned = true;
        }
    }

    pub fn census(&mut self, occupied: impl Fn(&[i32; 3]) -> bool) {
        for (cell, post) in &mut self.posts {
            post.occupied = occupied(cell);
            post.manned |= post.occupied;
        }
    }

    /// Whether no guard has held the post yet: the camp is being garrisoned for the first time,
    /// which nobody watches happen the way they would a replacement walking out of nowhere.
    pub fn fresh(&self, cell: &[i32; 3]) -> bool {
        self.posts.get(cell).is_some_and(|p| !p.manned)
    }

    /// The empty posts that may be filled now, in cell order.
    pub fn due(&self, now: u64) -> Vec<([i32; 3], PostMarker)> {
        self.posts
            .iter()
            .filter(|(_, p)| {
                !p.occupied
                    && now >= p.loaded_at + SETTLE
                    && p.died_at.is_none_or(|died| now >= died + RESPAWN)
            })
            .map(|(cell, p)| (*cell, p.marker))
            .collect()
    }
}

/// Whether a guard may appear at `point` without anyone watching it happen: some player is within
/// [`SPAWN_MAX`], none is within [`SPAWN_MIN`], and none within [`SIGHT`] `sees` it from their eye.
pub fn may_fill(
    point: [f64; 3],
    eyes: &[[f64; 3]],
    mut sees: impl FnMut([f64; 3], [f64; 3]) -> bool,
) -> bool {
    let dists: Vec<f64> = eyes.iter().map(|e| distance(*e, point)).collect();
    if !dists.iter().any(|d| *d <= SPAWN_MAX) || dists.iter().any(|d| *d < SPAWN_MIN) {
        return false;
    }
    eyes.iter()
        .zip(&dists)
        .all(|(eye, d)| *d > SIGHT || !sees(*eye, point))
}
