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

/// A replicated entity row with a stable identity.
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

/// A selection of rows out of a table shared by every recipient of a tick
/// window. The server builds each row ONCE per window no matter how many
/// connections track the entity; a recipient's set is a refcount bump plus
/// its own index list. On the wire it is just the selected rows, in order,
/// and a decoded set owns its rows outright.
pub struct RowSet<R> {
    table: Arc<[R]>,
    picks: Vec<u32>,
}

impl<R> RowSet<R> {
    /// The rows of `table` at `picks`, in `picks` order.
    ///
    /// # Panics
    /// If a pick is out of `table`'s range.
    pub fn select(table: Arc<[R]>, picks: Vec<u32>) -> Self {
        assert!(
            picks.iter().all(|&i| (i as usize) < table.len()),
            "row pick out of range"
        );
        RowSet { table, picks }
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
    /// Keep the rows `keep` accepts, letting it rewrite each in place — the
    /// transport's id remap. Detaches from the shared table.
    pub fn retain_mut(&mut self, keep: impl FnMut(&mut R) -> bool) {
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
        self.iter().eq(other.iter())
    }
}

impl<R: Serialize> Serialize for RowSet<R> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.iter())
    }
}

impl<'de, R: Deserialize<'de>> Deserialize<'de> for RowSet<R> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Vec::<R>::deserialize(deserializer).map(Into::into)
    }
}

/// One entity kind's replication to one recipient for one tick window.
/// Applied in field order: despawns, then spawns, then updates.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EntityLane<R, K> {
    /// Ids that left the recipient's interest since its last batch — out of
    /// range, or gone from the world. Sorted.
    pub despawned: Vec<K>,
    /// Full rows of entities that entered the recipient's interest this
    /// window: the client seeds them fresh (no interpolation from a previous
    /// row, animations at full weight).
    pub spawned: RowSet<R>,
    /// Rows of entities the recipient already tracks, as of this window.
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

/// Plain rows become UPDATES — the client treats an update for an id it does
/// not hold as a spawn, so a hand-built lane of rows reads naturally.
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
    /// Every row this lane carries: spawns first, then updates.
    pub fn iter(&self) -> impl Iterator<Item = &R> + '_ {
        self.spawned.iter().chain(self.updated.iter())
    }

    /// How many rows (spawns + updates) this lane carries.
    pub fn len(&self) -> usize {
        self.spawned.len() + self.updated.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0 && self.despawned.is_empty()
    }
}

impl<R: Clone, K> EntityLane<R, K> {
    /// The transport's id remap over both row lists (see
    /// [`RowSet::retain_mut`]). A dropped spawn's later updates drop the same
    /// way, and a despawn for an id the client never held is a no-op.
    pub fn retain_rows_mut(&mut self, mut keep: impl FnMut(&mut R) -> bool) {
        self.spawned.retain_mut(&mut keep);
        self.updated.retain_mut(&mut keep);
    }
}

impl<R: EntityRow> EntityLane<R, R::Id> {
    /// Fold the NEXT window's lane into this one, so applying the result
    /// equals applying both in order: ids despawned by `newer` lose their
    /// rows here, every row is the latest one per id, and an entity that
    /// (re)entered interest anywhere in the span stays a spawn. How the
    /// client collapses a backlog it cannot replay window by window.
    pub fn absorb(&mut self, newer: Self) {
        // Per id: the newest row, and whether it entered interest in the span.
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
