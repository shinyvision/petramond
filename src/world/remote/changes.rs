//! Which keys of the presented world changed when: its REVISION counter and
//! the keys in the order they last changed, so "everything that changed
//! after revision `r`" costs what changed, not what is present.
//!
//! Every replica write seam stamps the key it writes with the revision the
//! current frame will end on (`revision + 1`); the counter moves by exactly
//! one at the end of a frame that stamped anything. A key's stamp moves it to
//! the back of the order in O(1), and an unload unlinks it, so the order
//! never holds more than the present keys and needs no tombstones. Removals
//! are never journaled: `Presence` and `Population` are keys of their own,
//! stamped whenever the terrain or entity key set moves.

use mod_api::ClientStateKey;
use petramond_world::chunk::{ChunkPos, SectionPos};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::world::store::{for_each_column_cy, World};
use crate::world::{ReplicaWorld, WorldSide};

const NIL: u32 = u32::MAX;

struct Node {
    key: ClientStateKey,
    changed_at: u64,
    prev: u32,
    next: u32,
}

/// Present keys ordered by the revision they last changed at, oldest first:
/// an intrusive doubly linked list over a slab, indexed by key.
#[derive(Default)]
pub struct ChangeOrder {
    index: FxHashMap<ClientStateKey, u32>,
    nodes: Vec<Node>,
    free: Vec<u32>,
    head: u32,
    tail: u32,
}

impl ChangeOrder {
    pub fn new() -> Self {
        Self {
            head: NIL,
            tail: NIL,
            ..Default::default()
        }
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    pub fn contains(&self, key: &ClientStateKey) -> bool {
        self.index.contains_key(key)
    }

    /// The revision `key` last changed at, while it is present.
    pub fn changed_at(&self, key: &ClientStateKey) -> Option<u64> {
        self.index
            .get(key)
            .map(|&i| self.nodes[i as usize].changed_at)
    }

    fn unlink(&mut self, i: u32) {
        let (prev, next) = {
            let n = &self.nodes[i as usize];
            (n.prev, n.next)
        };
        match prev {
            NIL => self.head = next,
            p => self.nodes[p as usize].next = next,
        }
        match next {
            NIL => self.tail = prev,
            n => self.nodes[n as usize].prev = prev,
        }
    }

    fn push_back(&mut self, i: u32) {
        let tail = self.tail;
        {
            let n = &mut self.nodes[i as usize];
            n.prev = tail;
            n.next = NIL;
        }
        match tail {
            NIL => self.head = i,
            t => self.nodes[t as usize].next = i,
        }
        self.tail = i;
    }

    /// `key` changed at `revision`: it moves to the back. Stamps never go
    /// backwards, so the order stays sorted by `changed_at`.
    pub fn stamp(&mut self, key: ClientStateKey, revision: u64) {
        match self.index.get(&key) {
            Some(&i) => {
                self.nodes[i as usize].changed_at = revision;
                if self.tail != i {
                    self.unlink(i);
                    self.push_back(i);
                }
            }
            None => {
                let node = Node {
                    key,
                    changed_at: revision,
                    prev: NIL,
                    next: NIL,
                };
                let i = match self.free.pop() {
                    Some(i) => {
                        self.nodes[i as usize] = node;
                        i
                    }
                    None => {
                        self.nodes.push(node);
                        (self.nodes.len() - 1) as u32
                    }
                };
                self.index.insert(key, i);
                self.push_back(i);
            }
        }
    }

    /// `key` is no longer present.
    pub fn remove(&mut self, key: &ClientStateKey) -> bool {
        let Some(i) = self.index.remove(key) else {
            return false;
        };
        self.unlink(i);
        self.free.push(i);
        true
    }

    /// Every present key that changed after `revision`, newest first.
    pub fn since(&self, revision: u64) -> impl Iterator<Item = ClientStateKey> + '_ {
        let mut at = self.tail;
        std::iter::from_fn(move || {
            let node = self.nodes.get(at as usize)?;
            if node.changed_at <= revision {
                return None;
            }
            at = node.prev;
            Some(node.key)
        })
    }

    /// Every present key, in change order.
    pub fn keys(&self) -> impl Iterator<Item = ClientStateKey> + '_ {
        let mut at = self.head;
        std::iter::from_fn(move || {
            let node = self.nodes.get(at as usize)?;
            at = node.next;
            Some(node.key)
        })
    }
}

/// A presented world's revisions, and what this frame touched.
pub struct Changes {
    /// Where this world's revisions start: no other world's revision can
    /// equal one of these.
    base: u64,
    /// The revision the last finished frame left.
    revision: u64,
    /// Something was stamped since the last frame ended.
    stamped: bool,
    order: ChangeOrder,
    /// An events log wants this frame's touched and removed keys.
    collecting: bool,
    touched: Vec<ClientStateKey>,
    touched_set: FxHashSet<ClientStateKey>,
    removed: Vec<ClientStateKey>,
}

/// What one frame changed, handed to the events logs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameChanges {
    /// The revision the frame ended on.
    pub revision: u64,
    /// Every key a write seam wrote this frame, in first-write order.
    pub touched: Vec<ClientStateKey>,
    /// Sections and columns that unloaded this frame.
    pub removed: Vec<ClientStateKey>,
}

impl Default for Changes {
    fn default() -> Self {
        Self::new()
    }
}

impl Changes {
    pub fn new() -> Self {
        let base = petramond_world::world::data::revision_base();
        let mut changes = Self {
            base,
            revision: base,
            stamped: false,
            order: ChangeOrder::new(),
            collecting: false,
            touched: Vec::new(),
            touched_set: FxHashSet::default(),
            removed: Vec::new(),
        };
        // Present for the world's whole life, and never changed after.
        changes.stamp(ClientStateKey::Session);
        changes.stamp(ClientStateKey::Tables);
        changes
    }

    /// Where this world's revisions start: which world a revision is of.
    pub fn base(&self) -> u64 {
        self.base
    }

    /// The revision the last finished frame left: what a state taken now
    /// describes at least.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Whether this world issued `revision`: a base from another world, or
    /// one ahead of this counter, was not.
    pub fn issued(&self, revision: u64) -> bool {
        (self.base..=self.revision).contains(&revision)
    }

    pub fn order(&self) -> &ChangeOrder {
        &self.order
    }

    /// Collect touched and removed keys for the frames to come (an events
    /// log runs), or stop.
    pub fn set_collecting(&mut self, collecting: bool) {
        self.collecting = collecting;
        if !collecting {
            self.touched.clear();
            self.touched_set.clear();
            self.removed.clear();
        }
    }

    /// `key` changed this frame.
    pub fn stamp(&mut self, key: ClientStateKey) {
        self.order.stamp(key, self.revision + 1);
        self.stamped = true;
        if self.collecting && self.touched_set.insert(key) {
            self.touched.push(key);
        }
    }

    /// `key` left the world this frame (an unload or a despawn).
    pub fn remove(&mut self, key: ClientStateKey) {
        if !self.order.remove(&key) {
            return;
        }
        self.stamped = true;
        if self.collecting {
            if self.touched_set.remove(&key) {
                self.touched.retain(|k| *k != key);
            }
            if matches!(key, ClientStateKey::Section(_) | ClientStateKey::Column(_)) {
                self.removed.push(key);
            }
        }
    }

    /// A section or column arrived: it, and the terrain key set when it is
    /// new.
    pub fn stamp_terrain(&mut self, key: ClientStateKey) {
        if !self.order.contains(&key) {
            self.stamp(ClientStateKey::Presence);
        }
        self.stamp(key);
    }

    /// A section or column unloaded.
    pub fn remove_terrain(&mut self, key: ClientStateKey) {
        if self.order.contains(&key) {
            self.stamp(ClientStateKey::Presence);
            self.remove(key);
        }
    }

    /// End the frame: the counter moves when anything changed.
    pub fn end_frame(&mut self) -> FrameChanges {
        if std::mem::take(&mut self.stamped) {
            self.revision += 1;
        }
        self.touched_set.clear();
        FrameChanges {
            revision: self.revision,
            touched: std::mem::take(&mut self.touched),
            removed: std::mem::take(&mut self.removed),
        }
    }
}

pub fn section_key(pos: SectionPos) -> ClientStateKey {
    ClientStateKey::Section([pos.cx, pos.cy, pos.cz])
}

pub fn column_key(pos: ChunkPos) -> ClientStateKey {
    ClientStateKey::Column([pos.cx, pos.cz])
}

impl<S: WorldSide> World<S> {
    /// The sections holding at least one draw set.
    pub fn draw_sections(&self) -> impl Iterator<Item = SectionPos> + '_ {
        self.draws.block_draw_sections.keys().copied()
    }

    /// Section `pos` left the world. Only a replica keeps revisions: it is
    /// the world a client presents.
    pub(in crate::world) fn unstamp_section(&mut self, pos: SectionPos) {
        if let Some(replica) = self.side.replica_mut() {
            replica.changes.remove_terrain(section_key(pos));
        }
    }

    /// Column `pos` left the world, with every section in it.
    pub(in crate::world) fn unstamp_column(&mut self, pos: ChunkPos, cys: u32) {
        let Some(replica) = self.side.replica_mut() else {
            return;
        };
        for_each_column_cy(cys, |cy| {
            replica
                .changes
                .remove_terrain(section_key(SectionPos::new(pos.cx, cy, pos.cz)));
        });
        replica.changes.remove_terrain(column_key(pos));
    }
}

impl ReplicaWorld {
    /// The presented world's revisions.
    pub fn changes(&self) -> &Changes {
        &self.side.changes
    }

    pub fn changes_mut(&mut self) -> &mut Changes {
        &mut self.side.changes
    }

    /// The pool this world's background work runs on.
    pub fn job_pool(&self) -> &std::sync::Arc<crate::worker::JobPool> {
        self.side.terrain.prediction_terrain.pool()
    }

    /// Section `pos` is about to be written (installed, relit, edited,
    /// drawn on, or unloaded). A column a section install creates arrives
    /// with it.
    pub(in crate::world) fn stamp_section_write(&mut self, pos: SectionPos) {
        let changes = &mut self.side.changes;
        let column = column_key(pos.chunk_pos());
        if !changes.order.contains(&column) {
            changes.stamp_terrain(column);
        }
        changes.stamp_terrain(section_key(pos));
    }

    /// Column `pos` is about to be written.
    pub(in crate::world) fn stamp_column_write(&mut self, pos: ChunkPos) {
        self.side.changes.stamp_terrain(column_key(pos));
    }

    /// Cells were written outside the ingest seams (a local prediction and
    /// its reconciliation): their sections, and their columns' heights.
    pub fn stamp_cells_written(
        &mut self,
        cells: impl IntoIterator<Item = petramond_math::math::IVec3>,
    ) {
        for cell in cells {
            let Some(pos) = SectionPos::from_world(cell.x, cell.y, cell.z) else {
                continue;
            };
            if self.data.sections.contains_key(&pos) {
                self.side.changes.stamp(section_key(pos));
                self.side.changes.stamp(column_key(pos.chunk_pos()));
            }
        }
    }
}
