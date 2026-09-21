use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};

use petramond_world::chunk::{ChunkPos, SectionPos};

use super::worlds::player_path;
use super::{codec, colgen, region, LoadedColumnGen, LoadedSection, SectionSnapshot, SectionStore};

/// Messages from the game thread to the I/O thread.
pub(super) enum IoMsg {
    SaveSections {
        store: SectionStore,
        snaps: Vec<SectionSnapshot>,
    },
    SaveColumnGens(Vec<colgen::ColumnGenRecord>),
    SaveLevel(Vec<u8>),
    SavePlayer {
        name: String,
        bytes: Vec<u8>,
    },
    SaveModsJson(Vec<u8>),
    /// Writes that land on disk together or not at all (see `journal`).
    Batch(Vec<IoMsg>),
    Shutdown,
}

pub(super) enum ReadMsg {
    Section {
        pos: SectionPos,
        store: SectionStore,
        barrier: u64,
    },
    ColumnGen {
        pos: ChunkPos,
        seed: u32,
        barrier: u64,
    },
    Shutdown,
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

    fn read_record(&mut self, path: &Path, lidx: u16, barrier: u64) -> Option<Vec<u8>> {
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
            let reader = region::RegionReader::open(path).ok()?;
            (path.to_path_buf(), reader, barrier)
        };
        self.entries.push_back(entry);
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
        self.entries
            .back_mut()
            .and_then(|(_, reader, _)| reader.read_record(lidx).ok().flatten())
    }
}

/// How often a batch that failed to land is tried again while no new write
/// arrives to prompt it.
const RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// One write message's work, in the order it was queued.
struct Job {
    seq: u64,
    /// The authoritative part: lands whole through the journal.
    entries: Vec<super::journal::Entry>,
    /// Rebuildable caches riding the same message; written after the
    /// entries land, never journaled.
    caches: Vec<CacheWrite>,
}

enum CacheWrite {
    ExploredSections(Vec<SectionSnapshot>),
    ColumnGens(Vec<colgen::ColumnGenRecord>),
}

/// The write loop. Jobs land strictly in order: one that fails stays at the
/// head and is retried, later ones wait behind it in memory, and `completed`
/// only ever names a job whose writes are on disk — so a read barrier never
/// opens onto a record an unfinished write was about to replace. `held` is
/// the number of jobs waiting (0 = healthy).
pub(super) fn write_thread(
    dir: PathBuf,
    rx: Receiver<(u64, IoMsg)>,
    completed: Arc<(Mutex<u64>, Condvar)>,
    held: Arc<std::sync::atomic::AtomicU64>,
) {
    use std::sync::mpsc::RecvTimeoutError;
    crate::worker::lower_current_thread_priority();
    let explored_dir = dir.join("explored");
    let colgen_dir = dir.join("colgen");
    let mut jobs: VecDeque<Job> = VecDeque::new();
    let mut failing = false;
    let mut shutdown = false;
    while !shutdown {
        let received = if jobs.is_empty() {
            rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
        } else {
            rx.recv_timeout(RETRY_INTERVAL)
        };
        match received {
            Ok((seq, msg)) => {
                shutdown = matches!(msg, IoMsg::Shutdown);
                let mut job = Job {
                    seq,
                    entries: Vec::new(),
                    caches: Vec::new(),
                };
                split_job(msg, &mut job);
                if !jobs.is_empty() {
                    // Held jobs already cost memory; a cache rebuilds.
                    job.caches.clear();
                }
                jobs.push_back(job);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => shutdown = true,
        }
        while let Some(job) = jobs.front_mut() {
            if let Err(e) = super::journal::write(&dir, &job.entries) {
                if !failing {
                    log::error!(
                        "saving {} failed ({e}); holding the batch and retrying",
                        dir.display()
                    );
                }
                failing = true;
                break;
            }
            if failing {
                log::info!("saving {} works again", dir.display());
                failing = false;
            }
            for cache in job.caches.drain(..) {
                match cache {
                    CacheWrite::ExploredSections(snaps) => {
                        let _ = std::fs::create_dir_all(&explored_dir);
                        write_sections(&explored_dir, snaps);
                    }
                    CacheWrite::ColumnGens(recs) => {
                        let _ = std::fs::create_dir_all(&colgen_dir);
                        colgen::write_records(&colgen_dir, recs);
                    }
                }
            }
            *completed.0.lock().unwrap() = job.seq;
            completed.1.notify_all();
            jobs.pop_front();
        }
        held.store(jobs.len() as u64, std::sync::atomic::Ordering::Relaxed);
    }
    if !jobs.is_empty() {
        log::error!(
            "{} save batch(es) for {} never reached disk",
            jobs.len(),
            dir.display()
        );
    }
}

pub(super) fn read_thread(
    dir: PathBuf,
    rx: Receiver<ReadMsg>,
    load_tx: Sender<LoadedSection>,
    colgen_tx: Sender<LoadedColumnGen>,
    completed: Arc<(Mutex<u64>, Condvar)>,
) {
    crate::worker::lower_current_thread_priority();
    let mut decoders = super::decode::Decoders::new(load_tx, colgen_tx);
    let region_dir = dir.join("region");
    let explored_dir = dir.join("explored");
    let colgen_dir = dir.join("colgen");
    let mut region_cache = RegionFileCache::new(32);
    let mut colgen_cache = RegionFileCache::new(32);
    let mut pending = VecDeque::new();
    loop {
        let completed_seq = *completed.0.lock().unwrap();
        let ready = pending.iter().position(|msg| match msg {
            ReadMsg::Section { barrier, .. } | ReadMsg::ColumnGen { barrier, .. } => {
                *barrier <= completed_seq
            }
            ReadMsg::Shutdown => unreachable!("shutdown ends the loop on receipt"),
        });
        let msg = if let Some(index) = ready {
            pending
                .remove(index)
                .expect("ready read index came from this queue")
        } else {
            let received = if pending.is_empty() {
                rx.recv()
                    .map_err(|_| std::sync::mpsc::RecvTimeoutError::Disconnected)
            } else {
                rx.recv_timeout(std::time::Duration::from_millis(2))
            };
            match received {
                // Reads still waiting on a write that never landed have
                // nobody left to answer.
                Ok(ReadMsg::Shutdown) => break,
                Ok(msg) => {
                    pending.push_back(msg);
                    continue;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        };
        match msg {
            ReadMsg::Section {
                pos,
                store,
                barrier,
            } => {
                let (rx_, rz_) = region::region_of(pos);
                let source_dir = match store {
                    SectionStore::Authoritative => &region_dir,
                    SectionStore::ExploredCache => &explored_dir,
                };
                let path = region::region_path(source_dir, rx_, rz_);
                let bytes = region_cache.read_record(&path, region::local_index(pos), barrier);
                decoders.submit(super::decode::DecodeJob::Section { pos, store, bytes });
            }
            ReadMsg::ColumnGen { pos, seed, barrier } => {
                let (rx_, rz_) = colgen::region_of(pos);
                let path = colgen::cache_path(&colgen_dir, rx_, rz_);
                let bytes = colgen_cache.read_record(&path, colgen::local_index(pos), barrier);
                decoders.submit(super::decode::DecodeJob::Column { pos, seed, bytes });
            }
            ReadMsg::Shutdown => unreachable!("shutdown ends the loop on receipt"),
        }
    }
}

/// Sort a write message into the journal entries its authoritative parts
/// stand for and the cache writes riding along.
fn split_job(msg: IoMsg, job: &mut Job) {
    use super::journal::Entry;
    match msg {
        IoMsg::SaveSections {
            store: SectionStore::ExploredCache,
            snaps,
        } => job.caches.push(CacheWrite::ExploredSections(snaps)),
        IoMsg::SaveColumnGens(recs) => job.caches.push(CacheWrite::ColumnGens(recs)),
        IoMsg::SaveSections {
            store: SectionStore::Authoritative,
            snaps,
        } => {
            // Per region: each section's local index and its encoded record.
            type RegionRecords = Vec<(u16, Vec<u8>)>;
            let mut by_region: std::collections::BTreeMap<(i32, i32), RegionRecords> =
                Default::default();
            for s in &snaps {
                by_region
                    .entry(region::region_of(s.pos))
                    .or_default()
                    .push((region::local_index(s.pos), codec::encode_snapshot(s)));
            }
            job.entries.extend(
                by_region
                    .into_iter()
                    .map(|((rx, rz), records)| Entry::Region { rx, rz, records }),
            );
        }
        IoMsg::SaveLevel(bytes) => job.entries.push(Entry::File {
            path: "level.dat".into(),
            bytes,
        }),
        IoMsg::SavePlayer { name, bytes } => job.entries.push(Entry::File {
            path: player_path(Path::new("players"), &name)
                .to_string_lossy()
                .into_owned(),
            bytes,
        }),
        IoMsg::SaveModsJson(bytes) => job.entries.push(Entry::File {
            path: "mods.json".into(),
            bytes,
        }),
        IoMsg::Batch(msgs) => {
            for msg in msgs {
                split_job(msg, job);
            }
        }
        IoMsg::Shutdown => {}
    }
}

/// Merge cache snapshots into their region files (read-modify-write per region).
fn write_sections(region_dir: &Path, snaps: Vec<SectionSnapshot>) {
    use std::collections::HashMap;
    let mut by_region: HashMap<(i32, i32), Vec<SectionSnapshot>> = HashMap::new();
    for s in snaps {
        by_region
            .entry(region::region_of(s.pos))
            .or_default()
            .push(s);
    }
    for ((rx, rz), group) in by_region {
        let path = region::region_path(region_dir, rx, rz);
        let records = group
            .iter()
            .map(|s| (region::local_index(s.pos), codec::encode_snapshot(s)));
        let _ = region::merge_region(&path, records, region::MergePolicy::Rebuildable);
    }
}

#[cfg(test)]
mod tests;
