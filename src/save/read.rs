//! The read side of a world's save: several reader threads, each owning the
//! reads of the region files hashed to it, feeding the shared decoder pool.
//!
//! Sharding by region keeps every read of one region (and so every read of
//! one section) on one thread, in request order, while reads of different
//! regions overlap. A read waits for the write it must see (its barrier);
//! readers sleep on a condvar the writer signals whenever a write lands, so
//! a waiting read costs nothing and starts the moment its write is on disk.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;

use petramond_world::chunk::{ChunkPos, SectionPos};

use super::decode::{DecodeJob, Decoders};
use super::palette::Palette;
use super::{colgen, region, DecodedLoad, LoadedColumnGen, SectionStore};

/// One requested read, answered through the decoder pool.
pub(super) enum ReadMsg {
    Section {
        pos: SectionPos,
        store: SectionStore,
        /// The write sequence that must land before the record is read.
        barrier: u64,
    },
    ColumnGen {
        pos: ChunkPos,
        seed: u32,
        barrier: u64,
    },
}

impl ReadMsg {
    fn barrier(&self) -> u64 {
        match self {
            Self::Section { barrier, .. } | Self::ColumnGen { barrier, .. } => *barrier,
        }
    }

    /// The reader that owns this read: every read of one file lands on the
    /// same reader.
    fn shard(&self, shards: usize) -> usize {
        let ((rx, rz), salt) = match self {
            Self::Section { pos, .. } => (region::region_of(*pos), 0),
            Self::ColumnGen { pos, .. } => (colgen::region_of(*pos), 1),
        };
        let h = (i64::from(rx).wrapping_mul(73_856_093))
            ^ (i64::from(rz).wrapping_mul(19_349_663))
            ^ salt;
        h.rem_euclid(shards as i64) as usize
    }
}

struct ReadState {
    shards: Vec<VecDeque<ReadMsg>>,
    /// The newest write sequence known to be on disk.
    completed: u64,
    shutdown: bool,
}

/// The readers' inboxes plus the writer's completion mark, under one lock
/// and one condvar: a request, a landed write and shutdown all wake the
/// readers.
pub(super) struct ReadQueues {
    state: Mutex<ReadState>,
    wake: Condvar,
}

impl ReadQueues {
    pub(super) fn new(shards: usize) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(ReadState {
                shards: (0..shards.max(1)).map(|_| VecDeque::new()).collect(),
                completed: 0,
                shutdown: false,
            }),
            wake: Condvar::new(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, ReadState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn shard_count(&self) -> usize {
        self.lock().shards.len()
    }

    pub(super) fn request(&self, msg: ReadMsg) {
        let mut state = self.lock();
        let shard = msg.shard(state.shards.len());
        state.shards[shard].push_back(msg);
        drop(state);
        self.wake.notify_all();
    }

    /// The writer landed every write up to `seq`.
    pub(super) fn complete(&self, seq: u64) {
        self.lock().completed = seq;
        self.wake.notify_all();
    }

    pub(super) fn completed(&self) -> u64 {
        self.lock().completed
    }

    /// Stop the readers once they have run every read that is ready. Reads
    /// still waiting on a write that never landed have nobody left to answer.
    pub(super) fn shut_down(&self) {
        self.lock().shutdown = true;
        self.wake.notify_all();
    }

    /// The oldest ready read of `shard`, sleeping until there is one; `None`
    /// once shut down with nothing ready.
    fn next(&self, shard: usize) -> Option<ReadMsg> {
        let mut state = self.lock();
        loop {
            let completed = state.completed;
            let ready = state.shards[shard]
                .iter()
                .position(|msg| msg.barrier() <= completed);
            if let Some(index) = ready {
                return state.shards[shard].remove(index);
            }
            if state.shutdown {
                return None;
            }
            state = self.wake.wait(state).unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// How many reader threads a world gets: enough to overlap file reads on a
/// fast disk without competing with the decoders for cores.
pub(super) fn reader_count() -> usize {
    std::thread::available_parallelism().map_or(1, |n| (n.get() / 4).clamp(1, 4))
}

/// Start one reader per shard of `queues`, all feeding one decoder pool
/// that publishes to `sections` / `columns`. The pool shuts down when the
/// last reader exits.
pub(super) fn spawn_readers(
    dir: PathBuf,
    palette: Arc<Palette>,
    queues: Arc<ReadQueues>,
    sections: Sender<DecodedLoad>,
    columns: Sender<LoadedColumnGen>,
) -> Vec<JoinHandle<()>> {
    let decoders = Arc::new(Mutex::new(Decoders::new(
        dir.clone(),
        palette,
        sections,
        columns,
    )));
    (0..queues.shard_count())
        .map(|shard| {
            let (dir, queues, decoders) = (dir.clone(), queues.clone(), decoders.clone());
            std::thread::Builder::new()
                .name(format!("petramond-load-{shard}"))
                .spawn(move || read_loop(shard, &dir, &queues, &decoders))
                .expect("spawn save reader")
        })
        .collect()
}

fn read_loop(shard: usize, dir: &Path, queues: &ReadQueues, decoders: &Mutex<Decoders>) {
    crate::worker::lower_current_thread_priority();
    let region_dir = dir.join("region");
    let explored_dir = dir.join("explored");
    let colgen_dir = dir.join("colgen");
    let mut region_cache = RegionFileCache::new(32);
    let mut colgen_cache = RegionFileCache::new(32);
    while let Some(msg) = queues.next(shard) {
        let job = match msg {
            ReadMsg::Section {
                pos,
                store,
                barrier,
            } => {
                let (rx, rz) = region::region_of(pos);
                let source_dir = match store {
                    SectionStore::Authoritative => &region_dir,
                    SectionStore::ExploredCache => &explored_dir,
                };
                let path = region::region_path(source_dir, rx, rz);
                let bytes = region_cache.read_record(&path, region::local_index(pos), barrier);
                DecodeJob::Section { pos, store, bytes }
            }
            ReadMsg::ColumnGen { pos, seed, barrier } => {
                let (rx, rz) = colgen::region_of(pos);
                let path = colgen::cache_path(&colgen_dir, rx, rz);
                // A rebuildable cache: an unreadable record is simply a miss.
                let bytes = colgen_cache
                    .read_record(&path, colgen::local_index(pos), barrier)
                    .ok()
                    .flatten();
                DecodeJob::Column { pos, seed, bytes }
            }
        };
        decoders
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .submit(job);
    }
}

/// Open region readers retained by recency. Distance-ordered streaming crosses
/// region boundaries repeatedly, so a one-entry cache thrashes even though the
/// request set is spatially compact.
struct RegionFileCache {
    entries: VecDeque<(PathBuf, region::RegionReader, u64)>,
    capacity: usize,
}

impl RegionFileCache {
    fn new(capacity: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// The record's bytes, `Ok(None)` when the region file or the record is
    /// absent. Any other failure is an error: the record may exist. A reader
    /// opened before `barrier` landed is reopened, so it sees that write.
    fn read_record(
        &mut self,
        path: &Path,
        lidx: u16,
        barrier: u64,
    ) -> std::io::Result<Option<Vec<u8>>> {
        let entry = if let Some(i) = self
            .entries
            .iter()
            .position(|(p, _, epoch)| p == path && *epoch >= barrier)
        {
            self.entries
                .remove(i)
                .expect("cache position came from this deque")
        } else {
            if let Some(i) = self.entries.iter().position(|(p, _, _)| p == path) {
                self.entries.remove(i);
            }
            let reader = match region::RegionReader::open(path) {
                Ok(reader) => reader,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(e),
            };
            (path.to_path_buf(), reader, barrier)
        };
        self.entries.push_back(entry);
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
        let (_, reader, _) = self.entries.back_mut().expect("pushed above");
        reader.read_record(lidx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn section(x: i32, barrier: u64) -> ReadMsg {
        ReadMsg::Section {
            pos: SectionPos::new(x, 0, 0),
            store: SectionStore::Authoritative,
            barrier,
        }
    }

    fn cx(msg: Option<ReadMsg>) -> Option<i32> {
        match msg? {
            ReadMsg::Section { pos, .. } => Some(pos.cx),
            ReadMsg::ColumnGen { .. } => None,
        }
    }

    /// A read waits for its write, without polling: it is handed out the
    /// moment the writer marks that write landed, and ready reads keep their
    /// request order.
    #[test]
    fn a_read_waits_for_its_barrier_and_ready_reads_keep_their_order() {
        let queues = ReadQueues::new(1);
        queues.request(section(1, 5));
        queues.request(section(2, 0));
        queues.request(section(3, 0));
        assert_eq!(cx(queues.next(0)), Some(2), "the ready read goes first");
        assert_eq!(cx(queues.next(0)), Some(3));

        let reader = {
            let queues = queues.clone();
            std::thread::spawn(move || cx(queues.next(0)))
        };
        std::thread::sleep(Duration::from_millis(20));
        assert!(!reader.is_finished(), "still waiting on write 5");
        queues.complete(5);
        assert_eq!(reader.join().unwrap(), Some(1));
    }

    #[test]
    fn shutdown_runs_the_ready_reads_and_abandons_the_waiting_ones() {
        let queues = ReadQueues::new(1);
        queues.request(section(1, 9));
        queues.request(section(2, 0));
        queues.shut_down();
        assert_eq!(cx(queues.next(0)), Some(2));
        assert_eq!(cx(queues.next(0)), None);
    }

    #[test]
    fn every_read_of_one_region_lands_on_one_reader() {
        for shards in 1..=4 {
            for x in -64..64 {
                let a = section(x, 0).shard(shards);
                let same_region = section((x & !31) + 7, 0).shard(shards);
                assert_eq!(a, same_region, "x {x}, {shards} shards");
                assert!(a < shards);
            }
        }
    }
}
