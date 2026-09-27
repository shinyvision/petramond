//! Save layout, per-world dir in OS data dir.
//! level.dat: seed, tick, mod KV.
//! players/<key>.dat: pos, inventory, effects.
//! players/names.json: identity to display name.
//! region/ only keeps touched sections, rest regens from seed. Keeps saves small.
//!
//! None of this touches the 20 TPS loop. Deflate runs on the job pool during encode. One
//! writer thread does journal + io, a few reader threads sharded by region feed the decoder
//! pool. Game thread queues work and drains it in [`WorldSave::poll_loaded`] - mirrors the
//! section-gen pool.

pub mod client;
mod codec;
pub use petramond_worldgen::colgen;
mod container;
mod decode;
mod encode;
pub mod entities;
pub mod format;
mod furnace;
mod io;
mod journal;
pub mod level;
pub mod mobs;
pub mod palette;
pub mod player;
mod players;
mod read;
pub(crate) use petramond_region as region;
pub mod settings;
pub mod wire;
mod worlds;

#[cfg(test)]
mod tests;

pub use codec::{DiskSlot, KeptContent, SectionSnapshot};
pub use format::RecordError;
pub use level::LevelData;
pub use petramond_util::paths::base_data_dir;
pub use players::PlayerFiles;
pub use worlds::{
    delete_world, dir_name_for, list_worlds, random_seed, read_world_seed, read_world_settings,
    rename_world, seed_from_text, world_dir, world_exists, world_size_bytes, write_world_metadata,
    write_world_mod_baseline, write_world_settings, WorldInfo,
};

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crate::entity::DroppedItem;
use crate::mob::SavedMob;
use petramond_world::chunk::{ChunkPos, SectionPos};
use petramond_world::section::Section;

use crate::net::identity::PlayerKey;
use encode::EncodeSlot;
use io::{write_thread, IoMsg};
use read::{ReadMsg, ReadQueues};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SectionStore {
    Authoritative,
    ExploredCache,
}

pub struct LoadedSection {
    pub pos: SectionPos,
    pub store: SectionStore,
    pub record: SectionRecord,
}

pub enum SectionRecord {
    Absent,
    Decoded {
        section: Box<Section>,
        entities: Vec<DroppedItem>,
        mobs: Vec<SavedMob>,
    },
    Unreadable(Unreadable),
}

#[derive(Debug)]
pub struct Unreadable {
    pub error: RecordError,
    pub quarantined: Option<PathBuf>,
}

impl Unreadable {
    pub fn must_not_overwrite(&self) -> bool {
        self.error.is_from_newer_build() || self.quarantined.is_none()
    }
}

struct DecodedLoad {
    loaded: LoadedSection,
    kept: KeptContent,
}

pub struct LoadedColumnGen {
    pub pos: ChunkPos,
    pub record: Option<colgen::ColumnGenRecord>,
}

pub struct WorldSave {
    writes: Arc<WriteQueue>,
    reads: Arc<ReadQueues>,
    section_write_barriers: HashMap<(SectionStore, i32, i32), u64>,
    colgen_write_barriers: HashMap<(i32, i32), u64>,
    load_rx: Receiver<DecodedLoad>,
    colgen_rx: Receiver<LoadedColumnGen>,
    writer_handle: Option<JoinHandle<()>>,
    reader_handles: Vec<JoinHandle<()>>,
    colgen_manifest: rustc_hash::FxHashSet<ChunkPos>,
    /// Section coords whose written record currently carries live entities — dropped
    /// items OR mobs. A section leaves the set when re-saved with neither. The persist
    /// decision consults it so a section whose drops were picked up / despawned (or whose
    /// mobs wandered off, died, or distance-despawned) is rewritten to clear its
    /// now-stale record, instead of leaving the disk copy to resurrect them on the next
    /// load. Populated both when we save such a record and when we read one back (so
    /// cross-session staleness is seen).
    entities_on_disk: HashSet<SectionPos>,
    players: Arc<PlayerFiles>,
    dir: PathBuf,
    held_writes: Arc<AtomicU64>,
    write_protected: HashSet<SectionPos>,
    kept_sections: Mutex<HashMap<SectionPos, KeptContent>>,
    palette: Arc<palette::Palette>,
    encoders: Option<Arc<crate::worker::JobPool>>,
}

const ENCODE_PRIORITY: i64 = i64::MIN + 1;

struct WriteQueue {
    tx: Sender<(u64, IoMsg)>,
    next_seq: AtomicU64,
    open: Mutex<Option<OpenBatch>>,
}

struct OpenBatch {
    seq: u64,
    msgs: Vec<IoMsg>,
    guards: usize,
}

impl WriteQueue {
    fn next_seq(&self) -> u64 {
        self.next_seq.fetch_add(1, Ordering::AcqRel) + 1
    }

    fn queue(&self, msg: IoMsg) -> u64 {
        if let Some(batch) = self.open.lock().expect("save batch").as_mut() {
            batch.msgs.push(msg);
            return batch.seq;
        }
        let seq = self.next_seq();
        let _ = self.tx.send((seq, msg));
        seq
    }

    fn send_open_batch(&self) {
        if let Some(batch) = self.open.lock().expect("save batch").take() {
            let _ = self.tx.send((batch.seq, IoMsg::Batch(batch.msgs)));
        }
    }
}

#[must_use = "the batch is sent when this guard drops"]
pub struct SaveBatch {
    writes: Arc<WriteQueue>,
}

impl Drop for SaveBatch {
    fn drop(&mut self) {
        let last = {
            let mut open = self.writes.open.lock().expect("save batch");
            match open.as_mut() {
                Some(batch) => {
                    batch.guards -= 1;
                    batch.guards == 0
                }
                None => false,
            }
        };
        if last {
            self.writes.send_open_batch();
        }
    }
}

pub struct OpenedWorld {
    pub save: WorldSave,
    pub saved: crate::world::SavedIndex,
    pub level: Option<LevelData>,
    pub disabled_mods: std::collections::BTreeSet<String>,
    pub keep_inventory: bool,
    pub day_minutes: u32,
    pub missing_mods: Vec<String>,
}

fn missing_mods_backup_name(missing: &[String]) -> String {
    let ids: Vec<String> = missing
        .iter()
        .map(|id| {
            id.chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '-' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect()
        })
        .collect();
    format!("mods-missing-{}", ids.join("+"))
}

impl WorldSave {
    pub fn dir(&self) -> &std::path::Path {
        &self.dir
    }

    pub fn palette(&self) -> &palette::Palette {
        &self.palette
    }

    pub fn use_job_pool(&mut self, pool: Arc<crate::worker::JobPool>) {
        self.encoders = Some(pool);
    }

    fn queue_write(&self, msg: IoMsg) -> u64 {
        self.writes.queue(msg)
    }

    pub fn begin_batch(&self) -> SaveBatch {
        let mut open = self.writes.open.lock().expect("save batch");
        match open.as_mut() {
            Some(batch) => batch.guards += 1,
            None => {
                *open = Some(OpenBatch {
                    seq: self.writes.next_seq(),
                    msgs: Vec::new(),
                    guards: 1,
                })
            }
        }
        SaveBatch {
            writes: self.writes.clone(),
        }
    }

    pub fn held_writes(&self) -> u64 {
        self.held_writes.load(Ordering::Relaxed)
    }

    pub fn write_backlog(&self) -> u64 {
        self.writes
            .next_seq
            .load(Ordering::Acquire)
            .saturating_sub(self.reads.completed())
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
            let records = EncodeSlot::new(snaps);
            if let Some(pool) = &self.encoders {
                let (records, palette) = (records.clone(), self.palette.clone());
                pool.submit(ENCODE_PRIORITY, move || records.encode(&palette));
            }
            let seq = self.queue_write(IoMsg::SaveSections { store, records });
            self.section_write_barriers.insert((store, rx, rz), seq);
        }
    }

    pub fn record_holds_entities(&self, pos: SectionPos) -> bool {
        self.entities_on_disk.contains(&pos)
    }

    pub fn note_record_holds_entities(&mut self, pos: SectionPos) {
        self.entities_on_disk.insert(pos);
    }

    pub fn save_sections(
        &mut self,
        saved: &mut crate::world::SavedIndex,
        snaps: Vec<SectionSnapshot>,
    ) {
        if snaps.is_empty() {
            return;
        }
        let mut authoritative = Vec::new();
        let mut explored = Vec::new();
        let kept = self.kept_sections.lock().expect("kept sections");
        for mut s in snaps {
            if self.write_protected.contains(&s.pos) {
                continue;
            }
            if s.cache_only && !saved.authoritative_contains(s.pos) {
                saved.insert_explored(s.pos);
                explored.push(s);
                continue;
            }
            saved.insert_authoritative(s.pos);
            if let Some(kept) = kept.get(&s.pos) {
                s.kept = kept.clone();
            }
            if s.entities.is_empty() && s.mobs.is_empty() {
                self.entities_on_disk.remove(&s.pos);
            } else {
                self.entities_on_disk.insert(s.pos);
            }
            authoritative.push(s);
        }
        drop(kept);
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

    pub fn save_player(&self, key: &PlayerKey, player: &crate::player::Player) {
        if let Some(bytes) = self.players.encode_for_save(key, player) {
            self.queue_write(IoMsg::SavePlayer { key: *key, bytes });
        }
    }

    pub fn player_files(&self) -> Arc<PlayerFiles> {
        Arc::clone(&self.players)
    }

    pub fn load_player(&self, key: &PlayerKey) -> Result<Option<player::PlayerData>, RecordError> {
        self.players.load_player(key)
    }

    pub fn adopt_legacy_player(
        &self,
        name: &str,
        key: &PlayerKey,
    ) -> Result<Option<player::PlayerData>, RecordError> {
        self.players.adopt_legacy_player(name, key)
    }

    pub fn load_player_registry(&self) -> Option<Vec<u8>> {
        self.players.load_player_registry()
    }

    pub fn save_mods_json(&self, bytes: Vec<u8>) {
        self.queue_write(IoMsg::SaveModsJson(bytes));
    }

    pub fn request_load(
        &self,
        saved: &crate::world::SavedIndex,
        pos: SectionPos,
        use_explored_cache: bool,
    ) {
        let store = if saved.authoritative_contains(pos) {
            Some(SectionStore::Authoritative)
        } else if use_explored_cache && saved.explored_contains(pos) {
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
            self.reads.request(ReadMsg::Section {
                pos,
                store,
                barrier,
            });
        }
    }

    pub fn poll_loaded(&self) -> Option<LoadedSection> {
        let DecodedLoad { loaded, kept } = self.load_rx.try_recv().ok()?;
        let mut held = self.kept_sections.lock().expect("kept sections");
        if kept.is_empty() {
            held.remove(&loaded.pos);
        } else {
            held.insert(loaded.pos, kept);
        }
        Some(loaded)
    }

    pub fn note_section_load_miss(
        &mut self,
        saved: &mut crate::world::SavedIndex,
        pos: SectionPos,
        store: SectionStore,
    ) {
        match store {
            SectionStore::Authoritative => saved.remove_authoritative(pos),
            SectionStore::ExploredCache => saved.remove_explored(pos),
        }
    }

    pub fn note_section_unreadable(
        &mut self,
        saved: &mut crate::world::SavedIndex,
        pos: SectionPos,
        store: SectionStore,
        unreadable: &Unreadable,
    ) {
        self.note_section_load_miss(saved, pos, store);
        if store == SectionStore::Authoritative && unreadable.must_not_overwrite() {
            log::warn!(
                "section {pos:?} is write-protected this session ({}); changes there will not \
                 be saved",
                unreadable.error
            );
            self.write_protected.insert(pos);
        }
    }

    pub fn colgen_manifest_contains(&self, pos: ChunkPos) -> bool {
        self.colgen_manifest.contains(&pos)
    }

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

    pub fn request_column_gen(&self, pos: ChunkPos, seed: u32) {
        let barrier = self
            .colgen_write_barriers
            .get(&colgen::region_of(pos))
            .copied()
            .unwrap_or(0);
        self.reads
            .request(ReadMsg::ColumnGen { pos, seed, barrier });
    }

    pub fn poll_loaded_column_gen(&self) -> Option<LoadedColumnGen> {
        self.colgen_rx.try_recv().ok()
    }

    pub fn note_colgen_load_miss(&mut self, pos: ChunkPos) {
        self.colgen_manifest.remove(&pos);
    }

    pub fn shutdown(&mut self) {
        self.writes.send_open_batch();
        self.queue_write(IoMsg::Shutdown);
        if let Some(h) = self.writer_handle.take() {
            let _ = h.join();
        }
        self.reads.shut_down();
        for h in self.reader_handles.drain(..) {
            let _ = h.join();
        }
    }
}

impl Drop for WorldSave {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub fn open(name: &str) -> std::io::Result<OpenedWorld> {
    open_at(world_dir(name))
}

pub fn open_at(dir: PathBuf) -> std::io::Result<OpenedWorld> {
    let t0 = std::time::Instant::now();
    let region_dir = dir.join("region");
    let explored_dir = dir.join("explored");
    std::fs::create_dir_all(&region_dir)?;
    std::fs::create_dir_all(&explored_dir)?;
    format::prepare_world(&dir)?;
    if journal::recover(&dir)? {
        log::info!("finished an interrupted save in {}", dir.display());
    }
    let level = level::load(&dir)?;

    let world_settings = settings::load_persisting(&dir);
    let keep_inventory = world_settings.keep_inventory;
    let day_minutes = world_settings.day_minutes;
    let disabled_mods = world_settings.disabled_mods;

    let palette = Arc::new(palette::load_or_create(&dir, &disabled_mods)?);

    let missing_mods: Vec<String> = crate::modding::modset::check_at_open(&dir, &disabled_mods)
        .into_iter()
        .map(|entry| entry.id)
        .collect();
    if !missing_mods.is_empty() {
        format::back_up_small_files(&dir, &missing_mods_backup_name(&missing_mods))?;
    }

    let t_meta = t0.elapsed();

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
    files.extend(
        list_files(&region_dir)
            .into_iter()
            .map(|p| (Store::Authoritative, p)),
    );
    files.extend(
        list_files(&explored_dir)
            .into_iter()
            .map(|p| (Store::Explored, p)),
    );
    files.extend(
        list_files(&colgen_dir)
            .into_iter()
            .map(|p| (Store::Colgen, p)),
    );

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
                explored_manifest.extend(indices.iter().map(|&l| region::section_pos(rx, rz, l)));
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

    let players = Arc::new(PlayerFiles::new(dir.clone(), palette.clone()));
    let world_dir = dir.clone();
    let (tx, rx) = std::sync::mpsc::channel::<(u64, IoMsg)>();
    let (load_tx, load_rx) = std::sync::mpsc::channel::<DecodedLoad>();
    let (colgen_tx, colgen_rx) = std::sync::mpsc::channel::<LoadedColumnGen>();
    let reads = ReadQueues::new(read::reader_count());
    let held_writes = Arc::new(AtomicU64::new(0));
    let writer_handle = {
        let (dir, reads, held, palette) = (
            dir.clone(),
            reads.clone(),
            held_writes.clone(),
            palette.clone(),
        );
        std::thread::Builder::new()
            .name("petramond-save".to_string())
            .spawn(move || write_thread(dir, rx, reads, held, palette))
            .expect("spawn save writer")
    };
    let reader_handles =
        read::spawn_readers(dir, palette.clone(), reads.clone(), load_tx, colgen_tx);

    Ok(OpenedWorld {
        saved: crate::world::SavedIndex::from_scan(manifest, explored_manifest),
        save: WorldSave {
            writes: Arc::new(WriteQueue {
                tx,
                next_seq: AtomicU64::new(0),
                open: Mutex::new(None),
            }),
            reads,
            section_write_barriers: HashMap::new(),
            colgen_write_barriers: HashMap::new(),
            load_rx,
            colgen_rx,
            writer_handle: Some(writer_handle),
            reader_handles,
            colgen_manifest,
            entities_on_disk: HashSet::new(),
            players,
            dir: world_dir,
            held_writes,
            write_protected: HashSet::new(),
            kept_sections: Mutex::new(HashMap::new()),
            palette,
            encoders: None,
        },
        level,
        disabled_mods,
        keep_inventory,
        day_minutes,
        missing_mods,
    })
}
