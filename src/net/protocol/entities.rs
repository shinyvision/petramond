//! Per-recipient entity lanes: how one tick window's mob, dropped-item and
//! player rows reach ONE connection.
//!
//! Each connection holds an interest set per entity kind (server side, see
//! `server::game::interest`). A lane carries that set's CHANGES for the
//! window — the ids that left it, the rows of entities that entered it, and
//! the rows of entities still in it — so the client store persists an entity
//! from its spawn to its despawn instead of re-deriving the population from
//! every batch. A tracked id missing from both row lists is UNCHANGED (the
//! store holds its last row); today every tracked entity ships a full row
//! each window, and per-field deltas against those persisted rows can replace
//! the `updated` rows without touching spawn/despawn.
//!
//! A despawn carries no reason: dying is presented from the rows themselves
//! (`MobStateRow::dead` + ragdoll, `PlayerStateRow::alive`) while the entity
//! is still tracked, so leaving interest and being removed from the world
//! look the same to the recipient — the entity is gone from ITS view.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::player::PlayerId;

use super::{ItemStateRow, MobStateRow, PlayerStateRow};

pub trait EntityRow: Clone {
    type Id: Copy + Eq + Ord + std::fmt::Debug;
    fn entity_id(&self) -> Self::Id;
}

impl EntityRow for MobStateRow {
    type Id = u64;
    fn entity_id(&self) -> u64 {
        self.id
    }
}

impl EntityRow for ItemStateRow {
    type Id = u64;
    fn entity_id(&self) -> u64 {
        self.id
    }
}

impl EntityRow for PlayerStateRow {
    type Id = PlayerId;
    fn entity_id(&self) -> PlayerId {
        self.id
    }
}

/// Rows picked from a shared table. On the TCP transport a set can instead carry `packed`
/// per-field changes (`net::tick_delta`), which the receiving connection turns back into rows
/// before the message goes anywhere else.
pub struct RowSet<R> {
    table: Arc<[R]>,
    picks: Vec<u32>,
    packed: Vec<u8>,
}

impl<R> RowSet<R> {
    pub fn select(table: Arc<[R]>, picks: Vec<u32>) -> Self {
        assert!(
            picks.iter().all(|&i| (i as usize) < table.len()),
            "row pick out of range"
        );
        RowSet {
            table,
            picks,
            packed: Vec::new(),
        }
    }

    pub(crate) fn packed(bytes: Vec<u8>) -> Self {
        RowSet {
            table: Arc::from(Vec::new()),
            picks: Vec::new(),
            packed: bytes,
        }
    }

    pub(crate) fn take_packed(&mut self) -> Option<Vec<u8>> {
        (!self.packed.is_empty()).then(|| std::mem::take(&mut self.packed))
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = &R> + '_ {
        self.picks.iter().map(|&i| &self.table[i as usize])
    }

    pub fn len(&self) -> usize {
        self.picks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.picks.is_empty()
    }
}

impl<R: Clone> RowSet<R> {
    /// Rewrite rows in place when `keep` accepts them; that's the transport id remap trick.
    /// If we own the whole table in order, the rewrite happens in place and rejects just drop out
    /// of picks. A shared selection detaches from the table.
    pub fn retain_mut(&mut self, mut keep: impl FnMut(&mut R) -> bool) {
        let whole = self.picks.len() == self.table.len()
            && self.picks.iter().enumerate().all(|(i, &p)| p as usize == i);
        if whole {
            if let Some(rows) = Arc::get_mut(&mut self.table) {
                let kept: Vec<bool> = rows.iter_mut().map(&mut keep).collect();
                if kept.iter().any(|&k| !k) {
                    self.picks = (0..kept.len() as u32)
                        .filter(|&i| kept[i as usize])
                        .collect();
                }
                return;
            }
        }
        let mut rows: Vec<R> = self.iter().cloned().collect();
        rows.retain_mut(keep);
        *self = rows.into();
    }
}

impl<R> From<Vec<R>> for RowSet<R> {
    fn from(rows: Vec<R>) -> Self {
        let picks = (0..rows.len() as u32).collect();
        RowSet {
            table: rows.into(),
            picks,
            packed: Vec::new(),
        }
    }
}

impl<R> Default for RowSet<R> {
    fn default() -> Self {
        Vec::new().into()
    }
}

impl<R> Clone for RowSet<R> {
    fn clone(&self) -> Self {
        RowSet {
            table: Arc::clone(&self.table),
            picks: self.picks.clone(),
            packed: self.packed.clone(),
        }
    }
}

impl<R: std::fmt::Debug> std::fmt::Debug for RowSet<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl<R: PartialEq> PartialEq for RowSet<R> {
    fn eq(&self, other: &Self) -> bool {
        self.iter().eq(other.iter()) && self.packed == other.packed
    }
}

struct Rows<'a, R>(&'a RowSet<R>);

impl<R: Serialize> Serialize for Rows<'_, R> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.0.iter())
    }
}

impl<R: Serialize> Serialize for RowSet<R> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (Rows(self), &self.packed).serialize(serializer)
    }
}

impl<'de, R: Deserialize<'de>> Deserialize<'de> for RowSet<R> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (rows, packed) = <(Vec<R>, Vec<u8>)>::deserialize(deserializer)?;
        let mut set: RowSet<R> = rows.into();
        set.packed = packed;
        Ok(set)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EntityLane<R, K> {
    pub despawned: Vec<K>,
    pub spawned: RowSet<R>,
    pub updated: RowSet<R>,
}

impl<R, K> Default for EntityLane<R, K> {
    fn default() -> Self {
        EntityLane {
            despawned: Vec::new(),
            spawned: RowSet::default(),
            updated: RowSet::default(),
        }
    }
}

impl<R, K> From<Vec<R>> for EntityLane<R, K> {
    fn from(rows: Vec<R>) -> Self {
        EntityLane {
            despawned: Vec::new(),
            spawned: RowSet::default(),
            updated: rows.into(),
        }
    }
}

impl<R, K> EntityLane<R, K> {
    pub fn iter(&self) -> impl Iterator<Item = &R> + '_ {
        self.spawned.iter().chain(self.updated.iter())
    }

    pub fn len(&self) -> usize {
        self.spawned.len() + self.updated.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0 && self.despawned.is_empty()
    }
}

impl<R: Clone, K> EntityLane<R, K> {
    pub fn retain_rows_mut(&mut self, mut keep: impl FnMut(&mut R) -> bool) {
        self.spawned.retain_mut(&mut keep);
        self.updated.retain_mut(&mut keep);
    }
}

impl<R: EntityRow> EntityLane<R, R::Id> {
    pub fn apply_to(&self, set: &mut BTreeMap<R::Id, R>) {
        for id in &self.despawned {
            set.remove(id);
        }
        for row in self.iter() {
            set.insert(row.entity_id(), row.clone());
        }
    }

    /// Fold `newer`'s lane into this one, same effect as applying both in order.
    /// Ids `newer` despawns disappear here too, and rows collapse to the latest per id. An id that
    /// dropped out and came back during the span is still a spawn, not an update.
    /// That's what lets a client skip past a backlog it never got to replay window by window.
    pub fn absorb(&mut self, newer: Self) {
        let mut rows: BTreeMap<R::Id, (R, bool)> = BTreeMap::new();
        for row in self.spawned.iter() {
            rows.insert(row.entity_id(), (row.clone(), true));
        }
        for row in self.updated.iter() {
            rows.insert(row.entity_id(), (row.clone(), false));
        }
        for id in &newer.despawned {
            rows.remove(id);
            self.despawned.push(*id);
        }
        self.despawned.sort_unstable();
        self.despawned.dedup();
        for row in newer.spawned.iter() {
            rows.insert(row.entity_id(), (row.clone(), true));
        }
        for row in newer.updated.iter() {
            let spawned = rows.get(&row.entity_id()).is_some_and(|(_, s)| *s);
            rows.insert(row.entity_id(), (row.clone(), spawned));
        }
        let (mut spawned, mut updated) = (Vec::new(), Vec::new());
        for (row, entered) in rows.into_values() {
            if entered {
                spawned.push(row);
            } else {
                updated.push(row);
            }
        }
        self.spawned = spawned.into();
        self.updated = updated.into();
    }
}

pub type MobLane = EntityLane<MobStateRow, u64>;
pub type ItemLane = EntityLane<ItemStateRow, u64>;
pub type PlayerLane = EntityLane<PlayerStateRow, PlayerId>;
