//! Native world saving: a per-world directory under the OS data dir holding a
//! `level.dat` (seed, world tick, mod world KV), per-player `players/<name>.dat`
//! files (position, inventory, effects…), and `region/` files packing the
//! 16³ sections the player has modified. Everything else regenerates from the
//! seed, so a save stays small.
//!
//! Disk I/O (compression + file reads/writes) runs on a dedicated thread so the
//! 20 TPS game loop never blocks. The game thread sends snapshots / requests and
//! drains loaded sections via [`WorldSave::poll_loaded`], mirroring the section-gen
//! worker pool.

pub mod client;
mod codec;
pub mod colgen;
mod container;
pub mod entities;
mod furnace;
mod io;
pub mod level;
pub mod mobs;
pub(crate) mod palette;
pub mod player;
mod region;
pub mod settings;
mod torch;
mod worlds;

#[cfg(test)]
mod tests;

pub use codec::SectionSnapshot;
pub(crate) use io::write_atomic;
pub use level::LevelData;
pub(crate) use worlds::base_data_dir;
pub use worlds::{
    delete_world, dir_name_for, list_worlds, random_seed, read_world_settings, rename_world,
    seed_from_text, world_dir, world_exists, write_world_metadata, write_world_settings, WorldInfo,
};

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use crate::chunk::{ChunkPos, SectionPos};
use crate::entity::DroppedItem;
use crate::mob::SavedMob;
use crate::section::Section;

use io::{read_thread, write_thread, IoMsg, ReadMsg};
use worlds::player_path;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SectionStore {
    Authoritative,
    ExploredCache,
}

/// A section read back from disk (`section` is `None` if absent / corrupt) plus any
/// item entities and mobs stored in its record (empty if absent / corrupt / none
/// saved).
pub struct LoadedSection {
    pub pos: SectionPos,
    pub(crate) store: SectionStore,
    pub section: Option<Section>,
    pub entities: Vec<DroppedItem>,
    pub mobs: Vec<SavedMob>,
}

/// A column-gen cache record read back from disk (`record` is `None` when the
/// cache misses — absent, corrupt, or seed/version drift: regenerate instead).
pub struct LoadedColumnGen {
    pub pos: ChunkPos,
    pub record: Option<colgen::ColumnGenRecord>,
}

/// Live handle to a world's on-disk save and its I/O thread.
pub struct WorldSave {
    tx: Sender<(u64, IoMsg)>,
    read_tx: Sender<ReadMsg>,
    next_write_seq: AtomicU64,
    section_write_barriers: HashMap<(SectionStore, i32, i32), u64>,
    colgen_write_barriers: HashMap<(i32, i32), u64>,
    load_rx: Receiver<LoadedSection>,
    colgen_rx: Receiver<LoadedColumnGen>,
    writer_handle: Option<JoinHandle<()>>,
    reader_handle: Option<JoinHandle<()>>,
    /// Section coords present on disk: seeded at open from region headers, grown
    /// as we save. The load path consults it to choose overlay-from-disk vs
    /// regenerate.
    manifest: rustc_hash::FxHashSet<SectionPos>,
    /// Per-column view of `manifest`, so the streamer's per-column wanted-section
    /// scans don't walk the whole manifest for every column (that made vertical
    /// crossings O(columns × manifest) on a lived-in save).
    manifest_columns: rustc_hash::FxHashMap<ChunkPos, Vec<i32>>,
    /// Disposable full-section records created by Optimize explored terrain.
    /// They accelerate normal wanted windows but never widen them vertically.
    explored_manifest: rustc_hash::FxHashSet<SectionPos>,
    /// Columns with a column-gen cache record on disk ("Optimize explored
    /// terrain"): seeded at open from `colgen/` headers, grown as we save.
    /// Presence only — a hit still validates seed/version at decode.
    colgen_manifest: rustc_hash::FxHashSet<ChunkPos>,
    /// Section coords whose written record currently carries live entities — dropped
    /// items OR mobs. A section leaves the set when re-saved with neither. The persist
    /// decision consults it so a section whose drops were picked up / despawned (or whose
    /// mobs wandered off, died, or distance-despawned) is rewritten to clear its
    /// now-stale record, instead of leaving the disk copy to resurrect them on the next
    /// load. Populated both when we save such a record and when we read one back (so
    /// cross-session staleness is seen).
    entities_on_disk: HashSet<SectionPos>,
    /// `<world dir>/players/` — per-player `<sanitized name>.dat` files, read
    /// synchronously at session open/join (one small file, like `level.dat`).
    players_dir: PathBuf,
}

/// The result of opening (or creating) a world.
pub struct OpenedWorld {
    pub save: WorldSave,
    /// `Some` if a `level.dat` already existed (a returning world): seed, the
    /// world tick, and the mod world KV. Per-player state is NOT here — the
    /// session reads it per name via [`WorldSave::load_player`].
    pub level: Option<LevelData>,
    /// Mod pack ids disabled for THIS world (`settings.json`; empty = all
    /// enabled). Already applied to the palette here; the session applies it
    /// to the mod host / recipes / spawner.
    pub disabled_mods: std::collections::BTreeSet<String>,
}

impl WorldSave {
    fn queue_write(&self, msg: IoMsg) -> u64 {
        let seq = self.next_write_seq.fetch_add(1, Ordering::AcqRel) + 1;
        let _ = self.tx.send((seq, msg));
        seq
    }

    fn queue_section_writes(&mut self, store: SectionStore, snaps: Vec<SectionSnapshot>) {
        let mut by_region: HashMap<(i32, i32), Vec<SectionSnapshot>> = HashMap::new();
        for snap in snaps {
            by_region
                .entry(region::region_of(snap.pos))
                .or_default()
                .push(snap);
        }
        for ((rx, rz), snaps) in by_region {
            let seq = self.queue_write(IoMsg::SaveSections { store, snaps });
            self.section_write_barriers.insert((store, rx, rz), seq);
        }
    }

    /// `true` if either authoritative state or an explored cache record exists.
    pub fn manifest_contains(&self, pos: SectionPos) -> bool {
        self.manifest.contains(&pos) || self.explored_manifest.contains(&pos)
    }

    pub fn authoritative_manifest_contains(&self, pos: SectionPos) -> bool {
        self.manifest.contains(&pos)
    }

    pub fn explored_manifest_contains(&self, pos: SectionPos) -> bool {
        self.explored_manifest.contains(&pos)
    }

    pub fn manifest_sections_in_column(
        &self,
        pos: ChunkPos,
    ) -> impl Iterator<Item = SectionPos> + '_ {
        self.manifest_columns
            .get(&pos)
            .into_iter()
            .flatten()
            .map(move |&cy| SectionPos::new(pos.cx, cy, pos.cz))
    }

    /// `true` if `pos`'s written record currently carries live entities (dropped items
    /// or mobs) — so a save that now finds the section free of both must rewrite it, or
    /// the stale record resurrects them on the next load.
    pub fn record_holds_entities(&self, pos: SectionPos) -> bool {
        self.entities_on_disk.contains(&pos)
    }

    /// Note that `pos`'s on-disk record carries live entities (drops or mobs), learned
    /// by reading it back. Mirrors what [`save_sections`](Self::save_sections) records
    /// when it writes them, so a record saved in a *previous* session is still rewritten
    /// once its entities are gone.
    pub fn note_record_holds_entities(&mut self, pos: SectionPos) {
        self.entities_on_disk.insert(pos);
    }

    /// Queue modified sections for compression + region write (non-blocking).
    pub fn save_sections(&mut self, snaps: Vec<SectionSnapshot>) {
        if snaps.is_empty() {
            return;
        }
        let mut authoritative = Vec::new();
        let mut explored = Vec::new();
        for s in snaps {
            if s.cache_only && !self.manifest.contains(&s.pos) {
                self.explored_manifest.insert(s.pos);
                explored.push(s);
                continue;
            }
            if self.manifest.insert(s.pos) {
                self.manifest_columns
                    .entry(s.pos.chunk_pos())
                    .or_default()
                    .push(s.pos.cy);
            }
            // Track whether the record we're about to write carries any live entities —
            // drops or mobs (matching `encode_snapshot`'s FLAG_HAS_ENTITIES /
            // FLAG_HAS_MOBS). A section that loses them all is then re-saved once to clear
            // the record (see the persist decisions in `world::stream`/`world::store`).
            if s.entities.is_empty() && s.mobs.is_empty() {
                self.entities_on_disk.remove(&s.pos);
            } else {
                self.entities_on_disk.insert(s.pos);
            }
            authoritative.push(s);
        }
        if !authoritative.is_empty() {
            self.queue_section_writes(SectionStore::Authoritative, authoritative);
        }
        if !explored.is_empty() {
            self.queue_section_writes(SectionStore::ExploredCache, explored);
        }
    }

    pub fn save_level(&self, bytes: Vec<u8>) {
        self.queue_write(IoMsg::SaveLevel(bytes));
    }

    /// Queue a player-file write (`players/<sanitized name>.dat`, atomic like
    /// `level.dat`). `bytes` come from [`player::encode`].
    pub fn save_player(&self, name: &str, bytes: Vec<u8>) {
        self.queue_write(IoMsg::SavePlayer {
            name: name.to_string(),
            bytes,
        });
    }

    /// Blocking read of `players/<sanitized name>.dat` (`None` = no such
    /// player yet, or unreadable). Called once per player at session open/join
    /// time — one small file, synchronous like the `level.dat` read at open.
    pub fn load_player(&self, name: &str) -> Option<Vec<u8>> {
        std::fs::read(player_path(&self.players_dir, name)).ok()
    }

    /// Record the active mod set (`mods.json`) with the save — compared with a
    /// loud warning at the next open (`modding::modset`).
    pub fn save_mods_json(&self, bytes: Vec<u8>) {
        self.queue_write(IoMsg::SaveModsJson(bytes));
    }

    /// Ask the I/O thread to read `pos`; the result arrives via [`poll_loaded`].
    ///
    /// [`poll_loaded`]: Self::poll_loaded
    pub fn request_load(&self, pos: SectionPos, use_explored_cache: bool) {
        let store = if self.manifest.contains(&pos) {
            Some(SectionStore::Authoritative)
        } else if use_explored_cache && self.explored_manifest.contains(&pos) {
            Some(SectionStore::ExploredCache)
        } else {
            None
        };
        if let Some(store) = store {
            let (rx, rz) = region::region_of(pos);
            let barrier = self
                .section_write_barriers
                .get(&(store, rx, rz))
                .copied()
                .unwrap_or(0);
            let _ = self.read_tx.send(ReadMsg::Section {
                pos,
                store,
                barrier,
            });
        }
    }

    pub fn poll_loaded(&self) -> Option<LoadedSection> {
        self.load_rx.try_recv().ok()
    }

    /// A missing/corrupt record must not stay in the presence manifest or every
    /// revisit repeats the failed read and suppresses cache replacement.
    pub(crate) fn note_section_load_miss(&mut self, pos: SectionPos, store: SectionStore) {
        match store {
            SectionStore::Authoritative => {
                self.manifest.remove(&pos);
                if let Some(cys) = self.manifest_columns.get_mut(&pos.chunk_pos()) {
                    cys.retain(|&cy| cy != pos.cy);
                    if cys.is_empty() {
                        self.manifest_columns.remove(&pos.chunk_pos());
                    }
                }
            }
            SectionStore::ExploredCache => {
                self.explored_manifest.remove(&pos);
            }
        }
    }

    /// `true` if `pos` has a column-gen cache record on disk (or saved this
    /// session). A hit still validates seed/version at decode.
    pub fn colgen_manifest_contains(&self, pos: ChunkPos) -> bool {
        self.colgen_manifest.contains(&pos)
    }

    /// Queue explored columns' 2D gen data for the column-gen cache
    /// (non-blocking; "Optimize explored terrain").
    pub fn save_column_gens(&mut self, recs: Vec<colgen::ColumnGenRecord>) {
        if recs.is_empty() {
            return;
        }
        let mut by_region: HashMap<(i32, i32), Vec<colgen::ColumnGenRecord>> = HashMap::new();
        for rec in recs {
            self.colgen_manifest.insert(rec.pos);
            by_region
                .entry(colgen::region_of(rec.pos))
                .or_default()
                .push(rec);
        }
        for ((rx, rz), recs) in by_region {
            let seq = self.queue_write(IoMsg::SaveColumnGens(recs));
            self.colgen_write_barriers.insert((rx, rz), seq);
        }
    }

    /// Ask the I/O thread to read `pos`'s column-gen cache record (validated
    /// against `seed`); the result arrives via [`poll_loaded_column_gen`].
    ///
    /// [`poll_loaded_column_gen`]: Self::poll_loaded_column_gen
    pub fn request_column_gen(&self, pos: ChunkPos, seed: u32) {
        let barrier = self
            .colgen_write_barriers
            .get(&colgen::region_of(pos))
            .copied()
            .unwrap_or(0);
        let _ = self.read_tx.send(ReadMsg::ColumnGen { pos, seed, barrier });
    }

    pub fn poll_loaded_column_gen(&self) -> Option<LoadedColumnGen> {
        self.colgen_rx.try_recv().ok()
    }

    pub(crate) fn note_colgen_load_miss(&mut self, pos: ChunkPos) {
        self.colgen_manifest.remove(&pos);
    }

    /// Flush everything still queued and join the I/O thread. Call on quit after
    /// sending the final level / entities / chunks: the channel is ordered, so
    /// the join returns only once every prior write has hit disk.
    pub fn shutdown(&mut self) {
        self.queue_write(IoMsg::Shutdown);
        if let Some(h) = self.writer_handle.take() {
            let _ = h.join();
        }
        let _ = self.read_tx.send(ReadMsg::Shutdown);
        if let Some(h) = self.reader_handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for WorldSave {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Open (or create) a world's save directory and spin up its I/O thread.
pub fn open(name: &str) -> std::io::Result<OpenedWorld> {
    open_at(world_dir(name))
}

/// Open (or create) a world at an explicit directory. Backs [`open`]; tests use
/// it directly against a temp dir so they never touch the real data dir.
pub(crate) fn open_at(dir: PathBuf) -> std::io::Result<OpenedWorld> {
    let t0 = std::time::Instant::now();
    let region_dir = dir.join("region");
    let explored_dir = dir.join("explored");
    std::fs::create_dir_all(&region_dir)?;
    std::fs::create_dir_all(&explored_dir)?;

    // Per-world settings (`settings.json`; absent = defaults). Mod enablement
    // is read BEFORE the palette so disabled-mod content decodes as unknown.
    let world_settings = settings::load(&dir);
    let disabled_mods = world_settings.disabled_mods;

    // Pin (or load) the save's block/item name palette BEFORE any record is
    // read or written: the codec maps every id through it (see `palette`).
    palette::activate(&dir, &disabled_mods)?;

    // Compare the save's recorded mod set with the ENABLED one (loud warning
    // on any difference; never blocks — content degrades safely via the
    // palette). Deliberately disabled mods are not a mismatch.
    crate::modding::modset::warn_on_mismatch(&dir, &disabled_mods);

    let level = std::fs::read(dir.join("level.dat"))
        .ok()
        .and_then(|b| level::decode(&b));
    let t_meta = t0.elapsed();

    // Build the load manifests from existing region/cache headers. The record
    // headers interleave with the (small) compressed bodies, so an index scan
    // effectively streams the whole store through the page cache — a real
    // explored save is hundreds of MB across hundreds of files. The per-file
    // scans are independent, so ALL files of all three stores fan out over one
    // batch of scoped threads (short-lived; no persistent pool exists this
    // early in a session build).
    let t1 = std::time::Instant::now();
    let colgen_dir = dir.join("colgen");
    let list_files = |dir: &std::path::Path| -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .map(|rd| rd.flatten().map(|ent| ent.path()).collect())
            .unwrap_or_default()
    };
    #[derive(Copy, Clone)]
    enum Store {
        Authoritative,
        Explored,
        Colgen,
    }
    let mut files: Vec<(Store, PathBuf)> = Vec::new();
    files.extend(list_files(&region_dir).into_iter().map(|p| (Store::Authoritative, p)));
    files.extend(list_files(&explored_dir).into_iter().map(|p| (Store::Explored, p)));
    files.extend(list_files(&colgen_dir).into_iter().map(|p| (Store::Colgen, p)));

    enum ScanResult {
        Sections(Store, i32, i32, Vec<u16>),
        Columns(i32, i32, Vec<u16>),
    }
    let next_file = std::sync::atomic::AtomicUsize::new(0);
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(files.len())
        .max(1);
    let scanned: Vec<ScanResult> = std::thread::scope(|s| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                s.spawn(|| {
                    let mut out = Vec::new();
                    loop {
                        let i = next_file.fetch_add(1, Ordering::Relaxed);
                        let Some((store, path)) = files.get(i) else {
                            break out;
                        };
                        match store {
                            Store::Authoritative | Store::Explored => {
                                if let Some((rx, rz)) = region::parse_region_name(path) {
                                    if let Ok(indices) = region::read_region_indices(path) {
                                        out.push(ScanResult::Sections(*store, rx, rz, indices));
                                    }
                                }
                            }
                            Store::Colgen => {
                                if let Some((rx, rz)) = colgen::parse_cache_name(path) {
                                    if let Ok(indices) = colgen::read_cache_indices(path) {
                                        out.push(ScanResult::Columns(rx, rz, indices));
                                    }
                                }
                            }
                        }
                    }
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|w| w.join().expect("manifest scan worker"))
            .collect()
    });
    let mut manifest = rustc_hash::FxHashSet::default();
    let mut explored_manifest = rustc_hash::FxHashSet::default();
    let mut colgen_manifest = rustc_hash::FxHashSet::default();
    for result in scanned {
        match result {
            ScanResult::Sections(Store::Authoritative, rx, rz, indices) => {
                manifest.extend(indices.iter().map(|&l| region::section_pos(rx, rz, l)));
            }
            ScanResult::Sections(_, rx, rz, indices) => {
                explored_manifest
                    .extend(indices.iter().map(|&l| region::section_pos(rx, rz, l)));
            }
            ScanResult::Columns(rx, rz, indices) => {
                colgen_manifest.extend(indices.iter().map(|&l| colgen::column_pos(rx, rz, l)));
            }
        }
    }
    log::debug!(
        target: "petramond::join::perf",
        "save open: meta {:.1} ms, manifests {:.1} ms ({} auth, {} explored, {} colgen)",
        t_meta.as_secs_f64() * 1e3,
        t1.elapsed().as_secs_f64() * 1e3,
        manifest.len(),
        explored_manifest.len(),
        colgen_manifest.len()
    );

    let players_dir = dir.join("players");
    let (tx, rx) = std::sync::mpsc::channel::<(u64, IoMsg)>();
    let (read_tx, read_rx) = std::sync::mpsc::channel::<ReadMsg>();
    let (load_tx, load_rx) = std::sync::mpsc::channel::<LoadedSection>();
    let (colgen_tx, colgen_rx) = std::sync::mpsc::channel::<LoadedColumnGen>();
    let completed = Arc::new((Mutex::new(0u64), Condvar::new()));
    let writer_completed = completed.clone();
    let writer_dir = dir.clone();
    let writer_handle = std::thread::Builder::new()
        .name("petramond-save".to_string())
        .spawn(move || write_thread(writer_dir, rx, writer_completed))
        .expect("spawn save writer");
    let reader_handle = std::thread::Builder::new()
        .name("petramond-load".to_string())
        .spawn(move || read_thread(dir, read_rx, load_tx, colgen_tx, completed))
        .expect("spawn save reader");

    let mut manifest_columns: rustc_hash::FxHashMap<ChunkPos, Vec<i32>> =
        rustc_hash::FxHashMap::default();
    for sp in &manifest {
        manifest_columns
            .entry(sp.chunk_pos())
            .or_default()
            .push(sp.cy);
    }

    Ok(OpenedWorld {
        save: WorldSave {
            tx,
            read_tx,
            next_write_seq: AtomicU64::new(0),
            section_write_barriers: HashMap::new(),
            colgen_write_barriers: HashMap::new(),
            load_rx,
            colgen_rx,
            writer_handle: Some(writer_handle),
            reader_handle: Some(reader_handle),
            manifest,
            manifest_columns,
            explored_manifest,
            colgen_manifest,
            entities_on_disk: HashSet::new(),
            players_dir,
        },
        level,
        disabled_mods,
    })
}
