//! Order of file ops, no I/O. One queue per file, deps between ops, what a queue implies about file
//! size before it lands.
//!
//! Mutations on a file land in submission order, one waits on the previous. Different files run in
//! parallel. A read only waits on the last queued mutation overlapping its bytes. Delete/rename
//! wait on everything queued under them, and later ops under them wait on the dir op.

use std::collections::{BTreeMap, VecDeque};
use std::ops::Bound;
use std::sync::Arc;

use rustc_hash::FxHashMap;

pub(super) type OpId = u64;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(in crate::modding) struct FileKey {
    pub root: u64,
    pub folded: Arc<str>,
}

impl FileKey {
    fn ancestors(&self) -> impl Iterator<Item = FileKey> + '_ {
        self.folded.match_indices('/').map(|(at, _)| FileKey {
            root: self.root,
            folded: self.folded[..at].into(),
        })
    }
}

pub(super) fn under<V>(map: &BTreeMap<FileKey, V>, dir: &FileKey) -> Vec<FileKey> {
    if dir.folded.is_empty() {
        return map
            .range((Bound::Excluded(dir.clone()), Bound::Unbounded))
            .take_while(|(key, _)| key.root == dir.root)
            .map(|(key, _)| key.clone())
            .collect();
    }
    let bound = |last: char| FileKey {
        root: dir.root,
        folded: format!("{}{last}", dir.folded).into(),
    };
    map.range(bound('/')..bound('0'))
        .map(|(key, _)| key.clone())
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Effect {
    Grow(Option<u64>),
    Put {
        offset: u64,
        len: u64,
        truncate: bool,
    },
    Sync,
    Gone,
    Replace(Option<u64>),
}

struct Entry {
    op: OpId,
    effect: Effect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Disk {
    Unknown,
    Absent,
    Len(u64),
}

pub(super) struct FileState<H> {
    queue: VecDeque<Entry>,
    reads: BTreeMap<OpId, [u64; 2]>,
    pub disk: Disk,
    pub modified_ms: Option<u64>,
    pub error: Option<String>,
    pub handle: Option<H>,
    pub dirty: bool,
}

impl<H> Default for FileState<H> {
    fn default() -> Self {
        Self {
            queue: VecDeque::new(),
            reads: BTreeMap::new(),
            disk: Disk::Unknown,
            modified_ms: None,
            error: None,
            handle: None,
            dirty: false,
        }
    }
}

impl<H> FileState<H> {
    pub fn queued(&self) -> bool {
        !self.queue.is_empty()
    }

    pub fn projected(&self, base: Option<u64>) -> Option<u64> {
        self.queue
            .iter()
            .fold(base, |len, entry| match entry.effect {
                Effect::Grow(n) => Some(len.unwrap_or(0) + n.unwrap_or(0)),
                Effect::Put {
                    offset,
                    len: n,
                    truncate,
                } => Some(if truncate {
                    offset + n
                } else {
                    len.unwrap_or(0).max(offset + n)
                }),
                Effect::Sync => len,
                Effect::Gone => None,
                Effect::Replace(n) => n,
            })
    }

    fn last_touching(&self, start: u64, end: u64) -> Option<OpId> {
        let mut len = match self.disk {
            Disk::Len(n) => n,
            _ => 0,
        };
        let mut last = None;
        for entry in &self.queue {
            let hit = match entry.effect {
                Effect::Grow(n) => {
                    let from = len;
                    len += n.unwrap_or(0);
                    end > from
                }
                Effect::Put {
                    offset,
                    len: n,
                    truncate,
                } => {
                    let from = offset.min(len);
                    let hit = end > from && (truncate || start < offset + n);
                    len = if truncate {
                        offset + n
                    } else {
                        len.max(offset + n)
                    };
                    hit
                }
                Effect::Sync => false,
                Effect::Gone => {
                    len = 0;
                    true
                }
                Effect::Replace(n) => {
                    len = n.unwrap_or(0);
                    true
                }
            };
            if hit {
                last = Some(entry.op);
            }
        }
        last
    }
}

struct Op<J> {
    job: Option<J>,
    blockers: usize,
    dependents: Vec<OpId>,
    keys: Vec<FileKey>,
    running: bool,
}

pub(super) struct Sched<J, H> {
    next: OpId,
    ops: FxHashMap<OpId, Op<J>>,
    pub files: BTreeMap<FileKey, FileState<H>>,
    ready: VecDeque<OpId>,
}

impl<J, H> Default for Sched<J, H> {
    fn default() -> Self {
        Self {
            next: 1,
            ops: FxHashMap::default(),
            files: BTreeMap::new(),
            ready: VecDeque::new(),
        }
    }
}

impl<J, H> Sched<J, H> {
    pub fn file_mut(&mut self, key: &FileKey) -> &mut FileState<H> {
        self.files.entry(key.clone()).or_default()
    }

    fn last_of(&self, key: &FileKey) -> Option<OpId> {
        self.files
            .get(key)
            .and_then(|file| file.queue.back())
            .map(|entry| entry.op)
    }

    fn ancestor_deps(&self, key: &FileKey, deps: &mut Vec<OpId>) {
        deps.extend(key.ancestors().filter_map(|dir| self.last_of(&dir)));
    }

    fn subtree_deps(&self, key: &FileKey, deps: &mut Vec<OpId>) {
        for k in std::iter::once(key.clone()).chain(under(&self.files, key)) {
            if let Some(file) = self.files.get(&k) {
                deps.extend(file.queue.back().map(|entry| entry.op));
                deps.extend(file.reads.keys());
            }
        }
    }

    fn add(&mut self, job: J, mut deps: Vec<OpId>, blockers: usize, keys: Vec<FileKey>) -> OpId {
        let id = self.next;
        self.next += 1;
        deps.sort_unstable();
        deps.dedup();
        let mut blockers = blockers;
        for dep in deps {
            if let Some(op) = self.ops.get_mut(&dep) {
                op.dependents.push(id);
                blockers += 1;
            }
        }
        self.ops.insert(
            id,
            Op {
                job: Some(job),
                blockers,
                dependents: Vec::new(),
                keys,
                running: false,
            },
        );
        if blockers == 0 {
            self.ready.push_back(id);
        }
        id
    }

    pub fn mutate(&mut self, key: &FileKey, effect: Effect, job: J, held: bool) -> OpId {
        let mut deps: Vec<OpId> = self.last_of(key).into_iter().collect();
        self.ancestor_deps(key, &mut deps);
        if let (
            Effect::Put {
                offset,
                len,
                truncate,
            },
            Some(file),
        ) = (effect, self.files.get(key))
        {
            let end = if truncate { u64::MAX } else { offset + len };
            deps.extend(
                file.reads
                    .iter()
                    .filter(|(_, [start, stop])| *start < end && *stop > offset)
                    .map(|(op, _)| *op),
            );
        }
        let id = self.add(job, deps, usize::from(held), vec![key.clone()]);
        self.file_mut(key).queue.push_back(Entry { op: id, effect });
        id
    }

    pub fn relocate(&mut self, from: &FileKey, to: &FileKey, job: J) -> OpId {
        let mut deps = Vec::new();
        for key in [from, to] {
            self.subtree_deps(key, &mut deps);
            self.ancestor_deps(key, &mut deps);
        }
        let size = self.files.get(from).and_then(|file| match file.disk {
            Disk::Len(n) => file.projected(Some(n)),
            Disk::Absent => file.projected(None),
            Disk::Unknown => None,
        });
        let id = self.add(job, deps, 0, vec![from.clone(), to.clone()]);
        self.file_mut(from).queue.push_back(Entry {
            op: id,
            effect: Effect::Gone,
        });
        self.file_mut(to).queue.push_back(Entry {
            op: id,
            effect: Effect::Replace(size),
        });
        id
    }

    pub fn delete(&mut self, key: &FileKey, job: J) -> (OpId, Vec<J>) {
        let mut failed = Vec::new();
        for k in std::iter::once(key.clone()).chain(under(&self.files, key)) {
            let waiting: Vec<OpId> = self
                .files
                .get(&k)
                .into_iter()
                .flat_map(|file| file.reads.keys().copied())
                .filter(|op| self.ops.get(op).is_some_and(|op| !op.running))
                .collect();
            for op in waiting {
                self.ready.retain(|ready| *ready != op);
                failed.extend(self.ops.get_mut(&op).and_then(|op| op.job.take()));
                self.finish(op);
            }
        }
        let mut deps = Vec::new();
        for k in std::iter::once(key.clone()).chain(under(&self.files, key)) {
            deps.extend(self.last_of(&k));
        }
        self.ancestor_deps(key, &mut deps);
        let id = self.add(job, deps, 0, vec![key.clone()]);
        self.file_mut(key).queue.push_back(Entry {
            op: id,
            effect: Effect::Gone,
        });
        (id, failed)
    }

    pub fn read(&mut self, key: &FileKey, offset: u64, len: u64, job: J) -> OpId {
        let end = offset.saturating_add(len.max(1));
        let mut deps: Vec<OpId> = self
            .files
            .get(key)
            .and_then(|file| file.last_touching(offset, end))
            .into_iter()
            .collect();
        self.ancestor_deps(key, &mut deps);
        let id = self.add(job, deps, 0, vec![key.clone()]);
        self.file_mut(key).reads.insert(id, [offset, end]);
        id
    }

    pub fn list(&mut self, dir: &FileKey, job: J) -> OpId {
        let mut deps: Vec<OpId> = self.last_of(dir).into_iter().collect();
        self.ancestor_deps(dir, &mut deps);
        for key in under(&self.files, dir) {
            let queue = &self.files[&key].queue;
            deps.extend(queue.front().map(|entry| entry.op));
            deps.extend(
                queue
                    .iter()
                    .rev()
                    .find(|entry| matches!(entry.effect, Effect::Gone | Effect::Replace(_)))
                    .map(|entry| entry.op),
            );
        }
        let id = self.add(job, deps, 0, vec![dir.clone()]);
        self.file_mut(dir).reads.insert(id, [0, u64::MAX]);
        id
    }

    pub fn detached(&mut self, job: J) -> OpId {
        self.add(job, Vec::new(), 0, Vec::new())
    }

    pub fn queued_behind(&self, key: &FileKey, op: OpId) -> Vec<OpId> {
        self.files.get(key).map_or_else(Vec::new, |file| {
            file.queue
                .iter()
                .skip_while(|entry| entry.op != op)
                .skip(1)
                .map(|entry| entry.op)
                .collect()
        })
    }

    pub fn job_mut(&mut self, op: OpId) -> Option<&mut J> {
        self.ops.get_mut(&op).and_then(|op| op.job.as_mut())
    }

    pub fn release(&mut self, op: OpId, len: u64) {
        let Some(entry) = self.ops.get_mut(&op) else {
            return;
        };
        entry.blockers -= 1;
        if entry.blockers == 0 {
            self.ready.push_back(op);
        }
        for key in entry.keys.clone() {
            if let Some(file) = self.files.get_mut(&key) {
                for queued in file.queue.iter_mut().filter(|e| e.op == op) {
                    queued.effect = Effect::Grow(Some(len));
                }
            }
        }
    }

    pub fn take_ready(&mut self) -> Option<(OpId, J)> {
        while let Some(id) = self.ready.pop_front() {
            if let Some(op) = self.ops.get_mut(&id) {
                if let Some(job) = op.job.take() {
                    op.running = true;
                    return Some((id, job));
                }
            }
        }
        None
    }

    pub fn is_idle(&self) -> bool {
        self.ops.is_empty()
    }

    pub fn ready_len(&self) -> usize {
        self.ready.len()
    }

    pub fn finish(&mut self, id: OpId) {
        let Some(op) = self.ops.remove(&id) else {
            return;
        };
        for key in &op.keys {
            let Some(file) = self.files.get_mut(key) else {
                continue;
            };
            if file.queue.front().is_some_and(|entry| entry.op == id) {
                file.queue.pop_front();
            } else {
                file.queue.retain(|entry| entry.op != id);
            }
            file.reads.remove(&id);
            if file.queue.is_empty() {
                file.handle = None;
                if file.reads.is_empty()
                    && file.error.is_none()
                    && !matches!(file.disk, Disk::Len(_))
                {
                    self.files.remove(key);
                }
            }
        }
        for dependent in op.dependents {
            if let Some(waiting) = self.ops.get_mut(&dependent) {
                waiting.blockers -= 1;
                if waiting.blockers == 0 {
                    self.ready.push_back(dependent);
                }
            }
        }
    }

    pub fn subtree(&self, key: &FileKey) -> Vec<FileKey> {
        let mut keys = under(&self.files, key);
        keys.insert(0, key.clone());
        keys
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(path: &str) -> FileKey {
        FileKey {
            root: 0,
            folded: path.into(),
        }
    }

    fn ready(sched: &mut Sched<&'static str, ()>) -> Vec<&'static str> {
        std::iter::from_fn(|| sched.take_ready().map(|(_, job)| job)).collect()
    }

    fn landed(sched: &mut Sched<&'static str, ()>, key: &FileKey, len: u64) {
        sched.file_mut(key).disk = Disk::Len(len);
    }

    #[test]
    fn a_read_waits_only_for_the_queued_writes_it_overlaps() {
        let mut sched = Sched::<&'static str, ()>::default();
        let log = key("log");
        landed(&mut sched, &log, 100);
        let record = sched.mutate(&log, Effect::Grow(None), "record", true);
        let write = sched.mutate(
            &log,
            Effect::Put {
                offset: 10,
                len: 5,
                truncate: false,
            },
            "write",
            false,
        );
        sched.read(&log, 0, 10, "landed");
        sched.read(&log, 12, 1, "overlaps the write");
        sched.read(&log, 100, 4, "past the landed end");
        assert_eq!(ready(&mut sched), vec!["landed"]);

        sched.release(record, 8);
        assert_eq!(ready(&mut sched), vec!["record"]);
        sched.finish(record);
        assert_eq!(ready(&mut sched), vec!["write", "past the landed end"]);
        sched.finish(write);
        assert_eq!(ready(&mut sched), vec!["overlaps the write"]);
    }

    #[test]
    fn renames_and_deletes_are_ordered_after_queued_writes() {
        let mut sched = Sched::<&'static str, ()>::default();
        let (a, b) = (key("a"), key("b"));
        sched.file_mut(&a).disk = Disk::Absent;
        let write_a = sched.mutate(&a, Effect::Grow(Some(3)), "write a", false);
        let write_b = sched.mutate(&b, Effect::Grow(Some(3)), "write b", false);
        let rename = sched.relocate(&a, &b, "rename");
        assert_eq!(ready(&mut sched), vec!["write a", "write b"]);
        sched.finish(write_a);
        assert!(ready(&mut sched).is_empty());
        sched.finish(write_b);
        assert_eq!(ready(&mut sched), vec!["rename"]);
        assert_eq!(
            sched.files[&b].projected(Some(0)),
            Some(3),
            "the renamed file carries its projected size"
        );
        sched.finish(rename);

        let (dir, inner) = (key("dir"), key("dir/x"));
        let slow = sched.mutate(&inner, Effect::Grow(Some(1)), "inner write", false);
        sched.read(&inner, 0, 64, "waiting read");
        let (delete, failed) = sched.delete(&dir, "delete");
        assert_eq!(failed, vec!["waiting read"]);
        sched.mutate(&inner, Effect::Grow(Some(1)), "after delete", false);
        assert_eq!(ready(&mut sched), vec!["inner write"]);
        sched.finish(slow);
        assert_eq!(ready(&mut sched), vec!["delete"]);
        sched.finish(delete);
        assert_eq!(ready(&mut sched), vec!["after delete"]);
    }
}
