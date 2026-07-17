use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};

use crate::chunk::{ChunkPos, SectionPos};

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

/// The I/O thread loop: process requests in order, doing compression + file I/O
/// off the game loop. Returns (and so the join completes) on `Shutdown`.
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

pub(super) fn write_thread(
    dir: PathBuf,
    rx: Receiver<(u64, IoMsg)>,
    completed: Arc<(Mutex<u64>, Condvar)>,
) {
    crate::worker::lower_current_thread_priority();
    let region_dir = dir.join("region");
    let explored_dir = dir.join("explored");
    let colgen_dir = dir.join("colgen");
    let players_dir = dir.join("players");
    while let Ok((seq, msg)) = rx.recv() {
        let shutdown = matches!(msg, IoMsg::Shutdown);
        match msg {
            IoMsg::SaveSections { store, snaps } => {
                let target_dir = match store {
                    SectionStore::Authoritative => &region_dir,
                    SectionStore::ExploredCache => &explored_dir,
                };
                let _ = std::fs::create_dir_all(target_dir);
                write_sections(target_dir, snaps);
            }
            IoMsg::SaveColumnGens(recs) => {
                let _ = std::fs::create_dir_all(&colgen_dir);
                colgen::write_records(&colgen_dir, recs);
            }
            IoMsg::SaveLevel(bytes) => {
                let _ = write_atomic(&dir.join("level.dat"), &bytes);
            }
            IoMsg::SavePlayer { name, bytes } => {
                let _ = std::fs::create_dir_all(&players_dir);
                let _ = write_atomic(&player_path(&players_dir, &name), &bytes);
            }
            IoMsg::SaveModsJson(bytes) => {
                let _ = write_atomic(&dir.join("mods.json"), &bytes);
            }
            IoMsg::Shutdown => {}
        }
        let (lock, ready) = &*completed;
        *lock.lock().unwrap() = seq;
        ready.notify_all();
        if shutdown {
            break;
        }
    }
}

pub(super) fn read_thread(
    dir: PathBuf,
    rx: Receiver<ReadMsg>,
    load_tx: Sender<LoadedSection>,
    colgen_tx: Sender<LoadedColumnGen>,
    completed: Arc<(Mutex<u64>, Condvar)>,
) {
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
            ReadMsg::Shutdown => false,
        });
        let msg = if let Some(index) = ready {
            pending
                .remove(index)
                .expect("ready read index came from this queue")
        } else if matches!(pending.front(), Some(ReadMsg::Shutdown)) {
            break;
        } else {
            let received = if pending.is_empty() {
                rx.recv()
                    .map_err(|_| std::sync::mpsc::RecvTimeoutError::Disconnected)
            } else {
                rx.recv_timeout(std::time::Duration::from_millis(2))
            };
            match received {
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
                let decoded = region_cache
                    .read_record(&path, region::local_index(pos), barrier)
                    .and_then(|blob| codec::decode_section(pos, &blob));
                let (section, entities, mobs) = match decoded {
                    Some((section, entities, mobs)) => (Some(section), entities, mobs),
                    None => (None, Vec::new(), Vec::new()),
                };
                let _ = load_tx.send(LoadedSection {
                    pos,
                    store,
                    section,
                    entities,
                    mobs,
                });
            }
            ReadMsg::ColumnGen { pos, seed, barrier } => {
                let (rx_, rz_) = colgen::region_of(pos);
                let path = colgen::cache_path(&colgen_dir, rx_, rz_);
                let record = colgen_cache
                    .read_record(&path, colgen::local_index(pos), barrier)
                    .and_then(|blob| colgen::decode_record(pos, seed, &blob));
                let _ = colgen_tx.send(LoadedColumnGen { pos, record });
            }
            ReadMsg::Shutdown => unreachable!("shutdown is handled at the queue head"),
        }
    }
}

/// Merge snapshots into their region files (read-modify-write per region).
/// Returns the paths written, so the I/O thread's read cache can refresh.
fn write_sections(region_dir: &Path, snaps: Vec<SectionSnapshot>) -> Vec<PathBuf> {
    use std::collections::HashMap;
    let mut by_region: HashMap<(i32, i32), Vec<SectionSnapshot>> = HashMap::new();
    for s in snaps {
        by_region
            .entry(region::region_of(s.pos))
            .or_default()
            .push(s);
    }
    let mut touched = Vec::with_capacity(by_region.len());
    for ((rx, rz), group) in by_region {
        let path = region::region_path(region_dir, rx, rz);
        let records = group
            .iter()
            .map(|s| (region::local_index(s.pos), codec::encode_snapshot(s)));
        let _ = region::merge_region(&path, records);
        touched.push(path);
    }
    touched
}

/// Atomic file write: tmp + rename, so a crash mid-write can't truncate the live file.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}
