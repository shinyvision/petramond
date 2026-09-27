use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;

use petramond_world::chunk::SectionPos;

use super::encode::EncodeSlot;
use super::journal::Entry;
use super::palette::Palette;
use super::read::ReadQueues;
use super::worlds::player_path;
use super::{colgen, level, region, SectionStore};

type RegionRecords = HashMap<(i32, i32), Vec<(u16, Vec<u8>)>>;

pub(super) enum IoMsg {
    SaveSections {
        store: SectionStore,
        records: EncodeSlot,
    },
    SaveColumnGens(Vec<colgen::ColumnGenRecord>),
    SaveLevel(Vec<u8>),
    SavePlayer {
        key: crate::net::identity::PlayerKey,
        bytes: Vec<u8>,
    },
    SaveModsJson(Vec<u8>),
    Batch(Vec<IoMsg>),
    Shutdown,
}

const RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

struct Job {
    seq: u64,
    msgs: u64,
    entries: Vec<Entry>,
    caches: Vec<CacheWrite>,
}

enum CacheWrite {
    ExploredSections(Vec<(SectionPos, Vec<u8>)>),
    ColumnGens(Vec<colgen::ColumnGenRecord>),
}

impl Job {
    fn new() -> Self {
        Self {
            seq: 0,
            msgs: 0,
            entries: Vec::new(),
            caches: Vec::new(),
        }
    }

    fn absorb(&mut self, seq: u64, msg: IoMsg, dir: &Path, pal: &Palette, failing: bool) {
        self.seq = seq;
        self.msgs += 1;
        self.fold(msg, dir, pal, failing);
    }

    fn fold(&mut self, msg: IoMsg, dir: &Path, pal: &Palette, failing: bool) {
        match msg {
            IoMsg::SaveSections {
                store: SectionStore::ExploredCache,
                records,
            } => {
                if !failing {
                    self.caches
                        .push(CacheWrite::ExploredSections(records.take(pal)));
                }
            }
            IoMsg::SaveColumnGens(recs) => {
                if !failing {
                    self.caches.push(CacheWrite::ColumnGens(recs));
                }
            }
            IoMsg::SaveSections {
                store: SectionStore::Authoritative,
                records,
            } => {
                let mut by_region: RegionRecords = HashMap::new();
                for (pos, bytes) in records.take(pal) {
                    by_region
                        .entry(region::region_of(pos))
                        .or_default()
                        .push((region::local_index(pos), bytes));
                }
                for ((rx, rz), records) in by_region {
                    self.add_region(rx, rz, records);
                }
            }
            IoMsg::SaveLevel(bytes) => {
                if let Some(previous) = level::backup_bytes(dir) {
                    self.add_file(level::BACKUP.into(), previous);
                }
                self.add_file(level::FILE.into(), bytes);
            }
            IoMsg::SavePlayer { key, bytes } => self.add_file(
                player_path(Path::new("players"), &key)
                    .to_string_lossy()
                    .into_owned(),
                bytes,
            ),
            IoMsg::SaveModsJson(bytes) => self.add_file("mods.json".into(), bytes),
            IoMsg::Batch(msgs) => {
                for msg in msgs {
                    self.fold(msg, dir, pal, failing);
                }
            }
            IoMsg::Shutdown => {}
        }
    }

    fn add_region(&mut self, rx: i32, rz: i32, records: Vec<(u16, Vec<u8>)>) {
        let existing = self.entries.iter_mut().find_map(|entry| match entry {
            Entry::Region {
                rx: x,
                rz: z,
                records,
            } if (*x, *z) == (rx, rz) => Some(records),
            _ => None,
        });
        let Some(existing) = existing else {
            self.entries.push(Entry::Region { rx, rz, records });
            return;
        };
        let mut at: HashMap<u16, usize> = existing
            .iter()
            .enumerate()
            .map(|(i, (lidx, _))| (*lidx, i))
            .collect();
        for (lidx, bytes) in records {
            match at.get(&lidx) {
                Some(&i) => existing[i].1 = bytes,
                None => {
                    at.insert(lidx, existing.len());
                    existing.push((lidx, bytes));
                }
            }
        }
    }

    fn add_file(&mut self, path: String, bytes: Vec<u8>) {
        let existing = self.entries.iter_mut().find_map(|entry| match entry {
            Entry::File { path: p, bytes } if *p == path => Some(bytes),
            _ => None,
        });
        match existing {
            Some(existing) => *existing = bytes,
            None => self.entries.push(Entry::File { path, bytes }),
        }
    }
}

/// The write loop. Writes land strictly in order: a job that fails stays
/// pending and is retried, everything queued meanwhile merges into it, and
/// the readers' completion mark only ever names a write that is on disk —
/// so a read barrier never opens onto a record an unfinished write was
/// about to replace. `held` is the number of queued writes waiting behind
/// a failure (0 = healthy).
pub(super) fn write_thread(
    dir: PathBuf,
    rx: Receiver<(u64, IoMsg)>,
    reads: Arc<ReadQueues>,
    held: Arc<AtomicU64>,
    palette: Arc<Palette>,
) {
    crate::worker::lower_current_thread_priority();
    let explored_dir = dir.join("explored");
    let colgen_dir = dir.join("colgen");
    let mut pending: Option<Job> = None;
    let mut failing = false;
    let mut shutdown = false;
    while !shutdown {
        let first = match pending {
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            Some(_) => rx.recv_timeout(RETRY_INTERVAL),
        };
        let mut received = Vec::new();
        match first {
            Ok(msg) => received.push(msg),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => shutdown = true,
        }
        received.extend(rx.try_iter());
        for (seq, msg) in received {
            shutdown |= matches!(msg, IoMsg::Shutdown);
            pending
                .get_or_insert_with(Job::new)
                .absorb(seq, msg, &dir, &palette, failing);
        }
        let Some(job) = pending.as_ref() else {
            continue;
        };
        if let Err(e) = super::journal::write(&dir, &job.entries) {
            if !failing {
                log::error!(
                    "saving {} failed ({e}); holding the batch and retrying",
                    dir.display()
                );
            }
            failing = true;
            held.store(job.msgs, Ordering::Relaxed);
            continue;
        }
        if failing {
            log::info!("saving {} works again", dir.display());
            failing = false;
        }
        let job = pending.take().expect("checked above");
        for cache in job.caches {
            match cache {
                CacheWrite::ExploredSections(records) => {
                    let _ = std::fs::create_dir_all(&explored_dir);
                    write_cache_sections(&explored_dir, records);
                }
                CacheWrite::ColumnGens(recs) => {
                    let _ = std::fs::create_dir_all(&colgen_dir);
                    colgen::write_records(&colgen_dir, recs);
                }
            }
        }
        reads.complete(job.seq);
        held.store(0, Ordering::Relaxed);
    }
    if let Some(job) = pending {
        log::error!(
            "{} save write(s) for {} never reached disk",
            job.msgs,
            dir.display()
        );
    }
}

fn write_cache_sections(region_dir: &Path, records: Vec<(SectionPos, Vec<u8>)>) {
    let mut by_region: RegionRecords = HashMap::new();
    for (pos, bytes) in records {
        by_region
            .entry(region::region_of(pos))
            .or_default()
            .push((region::local_index(pos), bytes));
    }
    for ((rx, rz), records) in by_region {
        let path = region::region_path(region_dir, rx, rz);
        let _ = region::merge_region(&path, records, region::MergePolicy::Rebuildable);
    }
}

#[cfg(test)]
mod tests;
