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
pub mod level;
pub mod mobs;
pub(crate) mod palette;
pub mod player;
mod region;
pub mod settings;
mod torch;

pub use codec::SectionSnapshot;
pub use level::LevelData;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use crate::chunk::{ChunkPos, SectionPos};
use crate::entity::DroppedItem;
use crate::mob::SavedMob;
use crate::section::Section;

/// Messages from the game thread to the I/O thread.
enum IoMsg {
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

enum ReadMsg {
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
    manifest: HashSet<SectionPos>,
    /// Per-column view of `manifest`, so the streamer's per-column wanted-section
    /// scans don't walk the whole manifest for every column (that made vertical
    /// crossings O(columns × manifest) on a lived-in save).
    manifest_columns: HashMap<ChunkPos, Vec<i32>>,
    /// Disposable full-section records created by Optimize explored terrain.
    /// They accelerate normal wanted windows but never widen them vertically.
    explored_manifest: HashSet<SectionPos>,
    /// Columns with a column-gen cache record on disk ("Optimize explored
    /// terrain"): seeded at open from `colgen/` headers, grown as we save.
    /// Presence only — a hit still validates seed/version at decode.
    colgen_manifest: HashSet<ChunkPos>,
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
    /// The world's "Optimize explored terrain" setting (`settings.json`):
    /// persist all explored terrain + the column-gen cache for faster loads.
    pub optimize_explored_terrain: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorldInfo {
    /// User-facing world name. New worlds persist this in `world.json`; old worlds
    /// fall back to their save-directory name.
    pub name: String,
    /// Directory name under `<data>/saves/`, after path sanitization.
    pub dir_name: String,
    pub has_level: bool,
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

/// Base data dir: `~/.local/share/petramond` (Linux), `~/Library/Application
/// Support/petramond` (macOS), `%APPDATA%\petramond` (Windows). Falls back to
/// a hidden dir in the cwd if no home dir can be resolved. Also hosts the
/// user-installed mod pack root (`<data>/mods` — see `crate::assets`).
pub(crate) fn base_data_dir() -> PathBuf {
    directories::ProjectDirs::from("", "", "petramond")
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".petramond"))
}

fn saves_dir() -> PathBuf {
    base_data_dir().join("saves")
}

/// Directory for a named world: `<data>/saves/<name>/`.
pub fn world_dir(name: &str) -> PathBuf {
    saves_dir().join(sanitize(name))
}

/// The save-directory name a world NAME creates. Idempotent — a directory
/// name maps to itself, so open paths can take either at creation time. A
/// world's directory NEVER changes after creation (renames touch only the
/// `world.json` display name), so worlds must always be OPENED by
/// `WorldInfo::dir_name`, never by display name.
pub fn dir_name_for(name: &str) -> String {
    sanitize(name)
}

/// Reduce a world name to a single safe path component.
fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() {
        "world".to_string()
    } else {
        s
    }
}

/// The on-disk file for a player name: `players/<sanitized name>.dat`. Names
/// sanitize through the same routine as world save directories, so any display
/// name maps to a single safe path component.
fn player_path(players_dir: &Path, name: &str) -> PathBuf {
    players_dir.join(format!("{}.dat", sanitize(name)))
}

pub fn world_exists(name: &str) -> bool {
    world_dir(name).exists()
}

#[derive(serde::Deserialize, serde::Serialize)]
struct WorldMetadata {
    name: String,
}

pub fn write_world_metadata(name: &str) -> std::io::Result<()> {
    let dir = world_dir(name);
    std::fs::create_dir_all(&dir)?;
    let metadata = serde_json::to_vec(&WorldMetadata {
        name: name.trim().to_string(),
    })
    .map_err(std::io::Error::other)?;
    write_atomic(&dir.join("world.json"), &metadata)
}

pub fn list_worlds() -> std::io::Result<Vec<WorldInfo>> {
    let dir = saves_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };

    let mut worlds = Vec::new();
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if !kind.is_dir() {
            continue;
        }
        let dir_name = entry.file_name().to_string_lossy().to_string();
        let path = entry.path();
        let name = std::fs::read(path.join("world.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<WorldMetadata>(&bytes).ok())
            .map(|m| m.name)
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| dir_name.clone());
        worlds.push(WorldInfo {
            name,
            dir_name,
            has_level: path.join("level.dat").exists(),
        });
    }
    worlds.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
            .then_with(|| a.dir_name.cmp(&b.dir_name))
    });
    Ok(worlds)
}

/// Rename a world's DISPLAY name (`world.json`); the save directory keeps its
/// original name so nothing references a moved path. Client-mod data (minimap
/// exploration, waypoints) keys on the DIRECTORY name too, so it follows a
/// renamed world by construction — moving the directory would strand it.
pub fn rename_world(dir_name: &str, new_name: &str) -> std::io::Result<()> {
    if !is_single_path_component(dir_name) {
        return Err(std::io::Error::other("invalid world directory name"));
    }
    let new_name = new_name.trim();
    if new_name.is_empty() {
        return Err(std::io::Error::other("world name cannot be empty"));
    }
    let dir = saves_dir().join(dir_name);
    if !dir.is_dir() {
        return Err(std::io::Error::other("no such world"));
    }
    let metadata = serde_json::to_vec(&WorldMetadata {
        name: new_name.to_string(),
    })
    .map_err(std::io::Error::other)?;
    write_atomic(&dir.join("world.json"), &metadata)
}

pub fn delete_world(dir_name: &str) -> std::io::Result<()> {
    delete_world_at(&saves_dir(), dir_name)
}

/// Read a world's per-world settings by its save-directory name (the
/// world-select / World Settings screens address worlds this way). An invalid
/// or absent directory yields defaults (all mods enabled).
pub fn read_world_settings(dir_name: &str) -> settings::WorldSettings {
    if !is_single_path_component(dir_name) {
        return settings::WorldSettings::default();
    }
    settings::load(&saves_dir().join(dir_name))
}

/// Write a world's per-world settings by its save-directory name.
pub fn write_world_settings(
    dir_name: &str,
    settings: &settings::WorldSettings,
) -> std::io::Result<()> {
    if !is_single_path_component(dir_name) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "world directory must be a single path component",
        ));
    }
    settings::store(&saves_dir().join(dir_name), settings)
}

fn delete_world_at(saves: &Path, dir_name: &str) -> std::io::Result<()> {
    if !is_single_path_component(dir_name) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "world directory must be a single path component",
        ));
    }
    match std::fs::remove_dir_all(saves.join(dir_name)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn is_single_path_component(name: &str) -> bool {
    let mut components = Path::new(name).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

pub fn seed_from_text(text: &str) -> u32 {
    let text = text.trim();
    if let Ok(seed) = text.parse::<u32>() {
        return seed;
    }

    let mut hash = 0x811c_9dc5u32;
    for &b in text.as_bytes() {
        hash ^= b as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

pub fn random_seed() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut z = nanos ^ ((std::process::id() as u64) << 32);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    (z ^ (z >> 31)) as u32
}

/// Open (or create) a world's save directory and spin up its I/O thread.
pub fn open(name: &str) -> std::io::Result<OpenedWorld> {
    open_at(world_dir(name))
}

/// Open (or create) a world at an explicit directory. Backs [`open`]; tests use
/// it directly against a temp dir so they never touch the real data dir.
pub(crate) fn open_at(dir: PathBuf) -> std::io::Result<OpenedWorld> {
    let region_dir = dir.join("region");
    let explored_dir = dir.join("explored");
    std::fs::create_dir_all(&region_dir)?;
    std::fs::create_dir_all(&explored_dir)?;

    // Per-world settings (`settings.json`; absent = defaults). Mod enablement
    // is read BEFORE the palette so disabled-mod content decodes as unknown.
    let world_settings = settings::load(&dir);
    let disabled_mods = world_settings.disabled_mods;
    let optimize_explored_terrain = world_settings.optimize_explored_terrain;

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

    // Build the load manifest from existing region headers.
    let mut manifest = HashSet::new();
    if let Ok(rd) = std::fs::read_dir(&region_dir) {
        for ent in rd.flatten() {
            let path = ent.path();
            if let Some((rx, rz)) = region::parse_region_name(&path) {
                if let Ok(indices) = region::read_region_indices(&path) {
                    for lidx in indices {
                        manifest.insert(region::section_pos(rx, rz, lidx));
                    }
                }
            }
        }
    }

    let mut explored_manifest = HashSet::new();
    if let Ok(rd) = std::fs::read_dir(&explored_dir) {
        for ent in rd.flatten() {
            let path = ent.path();
            if let Some((rx, rz)) = region::parse_region_name(&path) {
                if let Ok(indices) = region::read_region_indices(&path) {
                    for lidx in indices {
                        explored_manifest.insert(region::section_pos(rx, rz, lidx));
                    }
                }
            }
        }
    }

    // The column-gen cache manifest ("Optimize explored terrain"), same shape.
    let mut colgen_manifest = HashSet::new();
    if let Ok(rd) = std::fs::read_dir(dir.join("colgen")) {
        for ent in rd.flatten() {
            let path = ent.path();
            if let Some((rx, rz)) = colgen::parse_cache_name(&path) {
                if let Ok(indices) = colgen::read_cache_indices(&path) {
                    for lidx in indices {
                        colgen_manifest.insert(colgen::column_pos(rx, rz, lidx));
                    }
                }
            }
        }
    }

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

    let mut manifest_columns: HashMap<ChunkPos, Vec<i32>> = HashMap::new();
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
        optimize_explored_terrain,
    })
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

fn write_thread(dir: PathBuf, rx: Receiver<(u64, IoMsg)>, completed: Arc<(Mutex<u64>, Condvar)>) {
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

fn read_thread(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::Block;
    use crate::item::{ItemStack, ItemType};
    use crate::mathh::Vec3;
    use crate::player::Player;

    fn temp_world_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("petramond-savetest-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn load_blocking(save: &WorldSave, pos: SectionPos) -> Option<LoadedSection> {
        save.request_load(pos, true);
        for _ in 0..500 {
            if let Some(l) = save.poll_loaded() {
                return Some(l);
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        None
    }

    /// Full disk round-trip through the I/O thread: write a modified section (with a
    /// resting item entity carrying a partly-elapsed lifetime) + level + a player
    /// file in one session, reopen in another, and read it all back. Item entities
    /// ride in the section record, so the drop returns when its section loads.
    #[test]
    fn save_reopen_roundtrips_section_level_entities() {
        let dir = temp_world_dir("roundtrip");
        let pos = SectionPos::new(5, -3, -9); // negative cy: below the old datum

        {
            let mut opened = open_at(dir.clone()).expect("open fresh");
            assert!(opened.level.is_none(), "fresh world has no level.dat");
            assert!(
                opened.save.load_player("Rachel S!").is_none(),
                "fresh world has no player files"
            );
            assert!(!opened.save.manifest_contains(pos));

            let mut section = Section::new(pos.cx, pos.cy, pos.cz);
            section.set_block(3, 0, 7, Block::Stone);
            section.set_water(3, 1, 7, Block::Water, 0x12);
            let mut snap = SectionSnapshot::from_section(&section);
            let mut drop = DroppedItem::new(
                Vec3::new(80.5, 70.0, -39.5),
                ItemStack::new(ItemType::Dirt, 9),
                1,
            );
            drop.ticks_lived = 2500;
            snap.entities.push(drop);
            opened.save.save_sections(vec![snap]);

            opened
                .save
                .save_level(level::encode(0xABCD, 4242, &Default::default()));

            // The player rides its own file, keyed by SANITIZED name — the
            // display name may contain anything.
            let mut plr = Player::new(Vec3::new(80.0, 70.0, -40.0));
            plr.inventory.set_active(4);
            opened.save.save_player("Rachel S!", player::encode(&plr));

            opened.save.shutdown(); // flush queued writes + join the I/O thread
        }

        {
            let opened = open_at(dir.clone()).expect("reopen");

            let level = opened.level.expect("level.dat restored");
            assert_eq!(level.seed, 0xABCD);
            assert_eq!(level.tick, 4242, "the world tick persists across sessions");

            let restored = opened
                .save
                .load_player("Rachel S!")
                .and_then(|b| player::decode(&b))
                .expect("player file restored under the same (sanitized) name");
            assert_eq!(restored.pos, Vec3::new(80.0, 70.0, -40.0));
            assert_eq!(restored.inventory.active_slot(), 4);

            assert!(
                opened.save.manifest_contains(pos),
                "manifest sees saved section"
            );

            let loaded = load_blocking(&opened.save, pos).expect("section loads from disk");
            let section = loaded.section.expect("section record decodes");
            assert_eq!(section.block_raw(3, 0, 7), Block::Stone.id());
            assert_eq!(section.block_raw(3, 1, 7), Block::Water.id());
            assert_eq!(section.water_meta(3, 1, 7), 0x12);

            // The item entity comes back with its section, lifetime intact.
            assert_eq!(loaded.entities.len(), 1);
            assert_eq!(loaded.entities[0].stack, ItemStack::new(ItemType::Dirt, 9));
            assert_eq!(
                loaded.entities[0].ticks_lived, 2500,
                "remaining lifetime persisted"
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explored_cache_does_not_expand_the_authoritative_manifest() {
        let dir = temp_world_dir("explored-cache");
        let cached_pos = SectionPos::new(5, -3, 9);
        let edited_pos = SectionPos::new(5, 4, 9);

        {
            let mut opened = open_at(dir.clone()).expect("open fresh");
            let mut cached = Section::new(cached_pos.cx, cached_pos.cy, cached_pos.cz);
            cached.set_block(2, 3, 4, Block::Stone);
            let mut cached_snap = SectionSnapshot::from_section(&cached);
            cached_snap.cache_only = true;

            let mut edited = Section::new(edited_pos.cx, edited_pos.cy, edited_pos.cz);
            edited.set_block(6, 7, 8, Block::Dirt);
            opened
                .save
                .save_sections(vec![cached_snap, SectionSnapshot::from_section(&edited)]);

            assert!(opened.save.explored_manifest_contains(cached_pos));
            assert!(!opened.save.authoritative_manifest_contains(cached_pos));
            assert_eq!(
                opened
                    .save
                    .manifest_sections_in_column(cached_pos.chunk_pos())
                    .collect::<Vec<_>>(),
                vec![edited_pos],
                "disposable cache sections must not widen the wanted vertical range"
            );
            opened.save.shutdown();
        }

        {
            let opened = open_at(dir.clone()).expect("reopen");
            assert!(opened.save.explored_manifest_contains(cached_pos));
            assert!(!opened.save.authoritative_manifest_contains(cached_pos));
            assert!(opened.save.authoritative_manifest_contains(edited_pos));
            let loaded = load_blocking(&opened.save, cached_pos).expect("cache section loads");
            assert_eq!(
                loaded
                    .section
                    .expect("cache record decodes")
                    .block_raw(2, 3, 4),
                Block::Stone.id()
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn seed_text_accepts_numbers_and_hashes_strings() {
        assert_eq!(seed_from_text("12345"), 12345);
        assert_eq!(seed_from_text(" 12345 "), 12345);
        assert_eq!(
            seed_from_text("petramond"),
            seed_from_text("petramond"),
            "string seeds are stable"
        );
        assert_ne!(
            seed_from_text("petramond"),
            seed_from_text("Petramond"),
            "different strings choose different compatible seeds"
        );
    }

    #[test]
    fn delete_world_removes_only_a_single_save_directory() {
        let saves = temp_world_dir("delete-world");
        let world = saves.join("My_World");
        std::fs::create_dir_all(world.join("region")).expect("create world dir");
        std::fs::write(world.join("level.dat"), b"level").expect("write level");

        delete_world_at(&saves, "My_World").expect("delete world");
        assert!(!world.exists(), "selected world directory is removed");

        let invalid = delete_world_at(&saves, "../outside").expect_err("reject nested path");
        assert_eq!(invalid.kind(), std::io::ErrorKind::InvalidInput);

        let _ = std::fs::remove_dir_all(&saves);
    }

    /// The unload/reload dupe, at the save layer: a section record written with a
    /// drop, then re-saved drop-free (the drop was picked up), must not bring the
    /// drop back on reload — and `record_holds_entities` must track the transition.
    #[test]
    fn re_saving_a_drop_free_section_clears_its_stale_record() {
        let dir = temp_world_dir("clear-stale-drops");
        let pos = SectionPos::new(2, 4, -4);

        let mut opened = open_at(dir.clone()).expect("open fresh");

        // Unload-with-item: the record is written carrying one drop.
        let mut section = Section::new(pos.cx, pos.cy, pos.cz);
        section.set_block(1, 0, 1, Block::Stone);
        let mut snap = SectionSnapshot::from_section(&section);
        snap.entities.push(DroppedItem::new(
            Vec3::new(33.0, 65.0, -63.0),
            ItemStack::new(ItemType::Dirt, 3),
            1,
        ));
        opened.save.save_sections(vec![snap]);
        assert!(
            opened.save.record_holds_entities(pos),
            "record now carries a drop"
        );

        let with_item = load_blocking(&opened.save, pos).expect("loads with item");
        assert_eq!(with_item.entities.len(), 1, "drop is present before pickup");

        // Pickup-then-unload: the section is re-saved with no drops. The channel is
        // ordered, so this write lands before the load below reads it back.
        let empty = SectionSnapshot::from_section(&section); // entities default to empty
        opened.save.save_sections(vec![empty]);
        assert!(
            !opened.save.record_holds_entities(pos),
            "rewrite cleared the flag"
        );

        let after = load_blocking(&opened.save, pos).expect("loads after pickup");
        assert!(
            after.entities.is_empty(),
            "the stale drop must not resurrect"
        );
        // The section's own edits survive the rewrite (only the drop was cleared).
        assert_eq!(
            after.section.expect("section decodes").block_raw(1, 0, 1),
            Block::Stone.id()
        );

        opened.save.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The same stale-record guard, for mobs: a section record written with a mob, then
    /// re-saved mob-free (the mob died, wandered off, or distance-despawned), must not
    /// bring the mob back on reload — and `record_holds_entities` must track it. The
    /// guard is one mechanism shared with dropped items, so this pins it for the mob
    /// path too.
    #[test]
    fn re_saving_a_mob_free_section_clears_its_stale_record() {
        let dir = temp_world_dir("clear-stale-mobs");
        let pos = SectionPos::new(-7, 4, 3);

        let mut opened = open_at(dir.clone()).expect("open fresh");

        // Unload-with-mob: the record is written carrying one mob.
        let section = Section::new(pos.cx, pos.cy, pos.cz);
        let mut snap = SectionSnapshot::from_section(&section);
        snap.mobs.push(crate::mob::SavedMob {
            kind: crate::mob::Mob::Owl,
            pos: Vec3::new(-100.5, 65.0, 56.5),
            yaw: 0.5,
            shear_regrow: 0,
            kv: Default::default(),
        });
        opened.save.save_sections(vec![snap]);
        assert!(
            opened.save.record_holds_entities(pos),
            "record now carries a mob"
        );

        let with_mob = load_blocking(&opened.save, pos).expect("loads with mob");
        assert_eq!(with_mob.mobs.len(), 1, "mob present before it leaves");

        // The mob is gone: the section is re-saved mob-free. The record must be rewritten
        // so the stale mob can't resurrect on the next load.
        let empty = SectionSnapshot::from_section(&section);
        opened.save.save_sections(vec![empty]);
        assert!(
            !opened.save.record_holds_entities(pos),
            "rewrite cleared the flag"
        );

        let after = load_blocking(&opened.save, pos).expect("loads after");
        assert!(after.mobs.is_empty(), "the stale mob must not resurrect");

        opened.save.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
