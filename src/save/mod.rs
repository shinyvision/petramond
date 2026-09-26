//! Native world saving: a per-world directory under the OS data dir holding a
//! `level.dat` (seed, world tick, mod world KV), per-player `players/<key>.dat`
//! files keyed by player identity (position, inventory, effects…), the
//! identity→display-name registry `players/names.json`, and `region/` files
//! packing the
//! 16³ sections the player has modified. Everything else regenerates from the
//! seed, so a save stays small.
//!
//! Nothing here blocks the 20 TPS game loop: section records deflate on the
//! shared job pool (see `encode`), one writer thread journals and applies
//! the writes (`io`), and a few reader threads sharded by region feed a
//! decoder pool (`read`, `decode`). The game thread queues snapshots and
//! requests and drains loaded sections via [`WorldSave::poll_loaded`],
//! mirroring the section-gen worker pool.

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
pub use worlds::{
    delete_world, dir_name_for, list_worlds, random_seed, read_world_seed, read_world_settings,
    rename_world, seed_from_text, world_dir, world_exists, world_size_bytes, write_world_metadata,
    write_world_settings, WorldInfo,
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
use worlds::{legacy_player_path, player_path};

/// The identity→display-name registry file inside `players/`.
const PLAYER_REGISTRY_FILE: &str = "names.json";

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SectionStore {
    Authoritative,
    ExploredCache,
}

/// A section read back from disk.
pub struct LoadedSection {
    pub pos: SectionPos,
    pub store: SectionStore,
    pub record: SectionRecord,
}

/// What a section read found. An unreadable record is never "absent": its
/// bytes may be the player's builds.
pub enum SectionRecord {
    /// No record on disk.
    Absent,
    /// The decoded section plus the item entities and mobs stored with it.
    Decoded {
        section: Box<Section>,
        entities: Vec<DroppedItem>,
        mobs: Vec<SavedMob>,
    },
    Unreadable(Unreadable),
}

/// A record that exists but could not be read.
#[derive(Debug)]
pub struct Unreadable {
    pub error: RecordError,
    /// Where the record's bytes were kept (`quarantine/` in the world
    /// directory), or `None` when they could not be kept (or could not be
    /// read in the first place).
    pub quarantined: Option<PathBuf>,
}

impl Unreadable {
    /// Whether writing over the record could lose data nobody kept: it came
    /// from a newer build, or its bytes are not in quarantine.
    pub fn must_not_overwrite(&self) -> bool {
        self.error.is_from_newer_build() || self.quarantined.is_none()
    }
}

/// A section read as the decoders publish it: the load, plus what its record
/// keeps that this build cannot bring to life, for the save to hold (see
/// [`WorldSave::poll_loaded`]).
struct DecodedLoad {
    loaded: LoadedSection,
    kept: KeptContent,
}

/// A column-gen cache record read back from disk (`record` is `None` when the
/// cache misses — absent, corrupt, or seed/version drift: regenerate instead).
pub struct LoadedColumnGen {
    pub pos: ChunkPos,
    pub record: Option<colgen::ColumnGenRecord>,
}

/// Live handle to a world's on-disk save and its I/O thread.
pub struct WorldSave {
    writes: Arc<WriteQueue>,
    reads: Arc<ReadQueues>,
    section_write_barriers: HashMap<(SectionStore, i32, i32), u64>,
    colgen_write_barriers: HashMap<(i32, i32), u64>,
    load_rx: Receiver<DecodedLoad>,
    colgen_rx: Receiver<LoadedColumnGen>,
    writer_handle: Option<JoinHandle<()>>,
    reader_handles: Vec<JoinHandle<()>>,
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
    /// `<world dir>/players/` — per-identity `<hex key>.dat` files, read
    /// synchronously at session open/join (one small file, like `level.dat`),
    /// plus the name registry.
    players_dir: PathBuf,
    /// The world's save directory.
    dir: PathBuf,
    /// Write jobs the I/O thread is holding because one failed to land.
    held_writes: Arc<AtomicU64>,
    /// Authoritative sections whose record could not be read and must not be
    /// overwritten (see [`Unreadable::must_not_overwrite`]): their saves are
    /// dropped for the rest of the session.
    write_protected: HashSet<SectionPos>,
    /// Player files under the same protection, by player identity.
    protected_players: Mutex<HashSet<PlayerKey>>,
    /// Content kept in disk form from each loaded section's record (mobs and
    /// item entities this build cannot bring to life — a removed or disabled
    /// mod's), written back with every save of the section so it returns
    /// with its mod. Replaced by each load of the section.
    kept_sections: Mutex<HashMap<SectionPos, KeptContent>>,
    /// The same for player files: slots and fields a live player cannot
    /// carry, by player identity.
    kept_players: Mutex<HashMap<PlayerKey, player::KeptPlayer>>,
    /// This world's name↔id palette. Every record the save writes or reads
    /// maps its ids through it (see [`palette`]).
    palette: Arc<palette::Palette>,
    /// The shared job pool section records deflate on (see `encode`);
    /// `None` until one is attached, when the writer encodes them itself.
    encoders: Option<Arc<crate::worker::JobPool>>,
}

/// Where section encoding sits among the pool's jobs (lower runs sooner):
/// just behind predicted terrain, ahead of every generation, light and mesh
/// job — a save's latency is what a crash would lose.
const ENCODE_PRIORITY: i64 = i64::MIN + 1;

/// The ordered lane to the write thread, shared with any open
/// [`SaveBatch`].
struct WriteQueue {
    tx: Sender<(u64, IoMsg)>,
    next_seq: AtomicU64,
    open: Mutex<Option<OpenBatch>>,
}

struct OpenBatch {
    seq: u64,
    msgs: Vec<IoMsg>,
    /// Live [`SaveBatch`] guards; the batch is sent when the last one drops.
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

/// While alive, every write queued on its [`WorldSave`] is gathered into one
/// batch that reaches disk whole or not at all; dropping it hands the batch
/// to the I/O thread. An unwind drops it too, so what was gathered is still
/// written and later writes are never swallowed by a batch nobody closes.
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

/// The result of opening (or creating) a world.
pub struct OpenedWorld {
    pub save: WorldSave,
    /// The scanned on-disk record index — WORLD DATA the sim consults (see
    /// `world::SavedIndex`); attach it to the world beside the save handle.
    pub saved: crate::world::SavedIndex,
    /// `Some` if a `level.dat` already existed (a returning world): seed, the
    /// world tick, and the mod world KV. Per-player state is NOT here — the
    /// session reads it per identity via [`WorldSave::load_player`].
    pub level: Option<LevelData>,
    /// Mod pack ids disabled for THIS world (`settings.json`; empty = all
    /// enabled). Already applied to the palette here; the session applies it
    /// to the mod host / recipes / spawner.
    pub disabled_mods: std::collections::BTreeSet<String>,
    /// Keep the inventory on death (`settings.json` world rule).
    pub keep_inventory: bool,
    /// Day length in real minutes (`settings.json`; night mirrors it — the
    /// session converts to cycle ticks).
    pub day_minutes: u32,
    /// Mods the world was last saved with that are not active now (see
    /// `modding::modset`). Their content is kept, and the world's small
    /// files were backed up before this open; the session can tell the
    /// player which mods to bring back.
    pub missing_mods: Vec<String>,
}

/// The backup directory for an open with `missing` mods: one per distinct
/// missing set, so reopening without the same mods keeps the first backup.
fn missing_mods_backup_name(missing: &[String]) -> String {
    let ids: Vec<String> = missing
        .iter()
        .map(|id| {
            id.chars()
                .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
                .collect()
        })
        .collect();
    format!("mods-missing-{}", ids.join("+"))
}

impl WorldSave {
    /// The world's save directory.
    pub fn dir(&self) -> &std::path::Path {
        &self.dir
    }

    /// This world's save palette.
    pub fn palette(&self) -> &palette::Palette {
        &self.palette
    }

    /// Deflate section records on `pool` (the world's shared job pool)
    /// instead of on the writer thread.
    pub fn use_job_pool(&mut self, pool: Arc<crate::worker::JobPool>) {
        self.encoders = Some(pool);
    }

    fn queue_write(&self, msg: IoMsg) -> u64 {
        self.writes.queue(msg)
    }

    /// Gather the writes queued while the returned guard lives into one
    /// batch. Only those: a write queued outside a batch (a section leaving
    /// the streamed area) lands on its own, so state that spans two records
    /// is consistent on disk as of the last batch, not between batches.
    /// Guards nest; the outermost one sends.
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

    /// How many queued writes are being held back because one failed to
    /// reach disk (it is retried; the rest merge into it). 0 = saving works.
    pub fn held_writes(&self) -> u64 {
        self.held_writes.load(Ordering::Relaxed)
    }

    /// How many queued writes have not reached disk yet — the save's lag
    /// behind the game. Healthy saving keeps this near zero; it grows while
    /// the disk is slow or failing (the writer merges the backlog, so its
    /// memory stays bounded by the distinct records written).
    pub fn write_backlog(&self) -> u64 {
        self.writes
            .next_seq
            .load(Ordering::Acquire)
            .saturating_sub(self.reads.completed())
    }

    /// One write per region group; each group starts deflating on the job
    /// pool right away (see `encode`).
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
    /// `saved` is the world's on-disk record index; this is one of its two
    /// write choke points (the other is [`note_section_load_miss`]).
    ///
    /// [`note_section_load_miss`]: Self::note_section_load_miss
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
            // Track whether the record we're about to write carries any live entities —
            // drops or mobs. A section that loses them all is then re-saved once to clear
            // the record (see the persist decisions in `world::stream`/`world::store`).
            // Kept content does not count: it rides every save of the section, so it
            // never needs a clearing rewrite.
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

    /// Encode `player` through this world's palette and queue its file
    /// write (`players/<hex key>.dat`, atomic like `level.dat`).
    ///
    /// A player whose file could not be read and was not kept aside is not
    /// written (see [`load_player`](Self::load_player)).
    pub fn save_player(&self, key: &PlayerKey, player: &crate::player::Player) {
        if self
            .protected_players
            .lock()
            .expect("protected players")
            .contains(key)
        {
            return;
        }
        let bytes = match self.kept_players.lock().expect("kept players").get(key) {
            Some(kept) => player::encode_keeping(player, &self.palette, kept),
            None => player::encode(player, &self.palette),
        };
        self.queue_write(IoMsg::SavePlayer { key: *key, bytes });
    }

    /// Blocking read and decode of `players/<hex key>.dat`: `Ok(None)` when
    /// the player has no file yet. Called once per player at session
    /// open/join time — one small file, synchronous like the `level.dat` read
    /// at open.
    ///
    /// A file that exists but does not decode is an error, never a fresh
    /// player: its bytes are copied to `quarantine/players/` first, and when
    /// that fails (or the file is from a newer build) the player's saves are
    /// dropped for the session so the original is never overwritten.
    pub fn load_player(&self, key: &PlayerKey) -> Result<Option<player::PlayerData>, RecordError> {
        let path = player_path(&self.players_dir, key);
        let Some(bytes) = self.read_player_file(&path, key)? else {
            return Ok(None);
        };
        self.decode_player_file(&path, &bytes, key).map(Some)
    }

    /// Hand a pre-identity `players/<sanitized name>.dat` to `key`: once it
    /// decodes, the file MOVES to `key`'s own path (synchronously, before
    /// anything else can claim it) and its contents are returned. `Ok(None)`
    /// = no legacy file for that name, or `key` already has a file of its
    /// own (never overwritten). Worlds saved before player identities existed
    /// migrate one player at a time this way, on that name's first
    /// authenticated claim.
    ///
    /// A legacy file that cannot be read or decoded is an error exactly like
    /// [`load_player`](Self::load_player): it is quarantined and never
    /// moved, and when it could not be kept aside (or is from a newer build)
    /// `key`'s saves are dropped for the session, so no fresh player file
    /// takes its place.
    pub fn adopt_legacy_player(
        &self,
        name: &str,
        key: &PlayerKey,
    ) -> Result<Option<player::PlayerData>, RecordError> {
        let legacy = legacy_player_path(&self.players_dir, name);
        let owned = player_path(&self.players_dir, key);
        if owned.exists() {
            return Ok(None);
        }
        let Some(bytes) = self.read_player_file(&legacy, key)? else {
            return Ok(None);
        };
        let data = self.decode_player_file(&legacy, &bytes, key)?;
        if let Err(e) = std::fs::rename(&legacy, &owned) {
            // The decoded state still restores; the player's next save lands
            // at `owned` and the legacy file is left as it was.
            log::warn!(
                "could not migrate legacy player file {}: {e}",
                legacy.display()
            );
            return Ok(Some(data));
        }
        if let Err(e) = petramond_util::atomic_file::sync_dir(&self.players_dir) {
            log::warn!("could not sync the players directory: {e}");
        }
        Ok(Some(data))
    }

    /// Read a player file: `Ok(None)` when it does not exist. Any other read
    /// failure protects `key`'s saves (nothing of it could be kept aside).
    fn read_player_file(
        &self,
        path: &std::path::Path,
        key: &PlayerKey,
    ) -> Result<Option<Vec<u8>>, RecordError> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => {
                log::error!("could not read player file {}: {e}", path.display());
                self.protect_player(key);
                Err(RecordError::Io {
                    format: player::FORMAT.name,
                    kind: e.kind(),
                })
            }
        }
    }

    /// Decode a player file's bytes; an unreadable one is quarantined and,
    /// when it must not be overwritten, protects `key`'s saves.
    fn decode_player_file(
        &self,
        path: &std::path::Path,
        bytes: &[u8],
        key: &PlayerKey,
    ) -> Result<player::PlayerData, RecordError> {
        let error = match player::decode(bytes, &self.palette) {
            Ok(data) => {
                let mut kept = self.kept_players.lock().expect("kept players");
                if data.kept.is_empty() {
                    kept.remove(key);
                } else {
                    kept.insert(*key, data.kept.clone());
                }
                return Ok(data);
            }
            Err(error) => error,
        };
        let relative = std::path::Path::new("players").join(path.file_name().unwrap_or_default());
        let unreadable = Unreadable {
            quarantined: format::quarantine(&self.dir, &relative, bytes)
                .inspect_err(|e| log::error!("could not quarantine {}: {e}", path.display()))
                .ok(),
            error,
        };
        log::error!(
            "player file {} is unreadable ({}); kept at {:?}",
            path.display(),
            unreadable.error,
            unreadable.quarantined
        );
        if unreadable.must_not_overwrite() {
            self.protect_player(key);
        }
        Err(unreadable.error)
    }

    fn protect_player(&self, key: &PlayerKey) {
        log::warn!("player {key} will not be saved this session: its file must not be overwritten");
        self.protected_players
            .lock()
            .expect("protected players")
            .insert(*key);
    }

    /// The identity→display-name registry bytes (`None` = none saved yet).
    pub fn load_player_registry(&self) -> Option<Vec<u8>> {
        std::fs::read(self.players_dir.join(PLAYER_REGISTRY_FILE)).ok()
    }

    /// Replace the identity→display-name registry, synchronously and
    /// atomically: it changes only when a player first joins or renames, and
    /// it must never lag the player files it describes.
    pub fn store_player_registry(&self, bytes: &[u8]) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.players_dir)?;
        petramond_util::atomic_file::replace(&self.players_dir.join(PLAYER_REGISTRY_FILE), bytes)
    }

    /// Record the active mod set (`mods.json`) with the save — compared with a
    /// loud warning at the next open (`modding::modset`).
    pub fn save_mods_json(&self, bytes: Vec<u8>) {
        self.queue_write(IoMsg::SaveModsJson(bytes));
    }

    /// Ask the I/O thread to read `pos`; the result arrives via [`poll_loaded`].
    ///
    /// [`poll_loaded`]: Self::poll_loaded
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

    /// The next finished section read. What its record keeps in disk form
    /// stays with the save (see `kept_sections`), so world code only ever
    /// sees content it can bring to life.
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

    /// A missing record must not stay in the presence index or every revisit
    /// repeats the failed read and suppresses cache replacement.
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

    /// An unreadable record leaves the presence index like a missing one (so
    /// generation stands in for it), but an authoritative record that must
    /// not be overwritten also becomes write-protected: the generated
    /// stand-in is never saved over it this session. A quarantined corrupt
    /// record may be replaced — its bytes are kept.
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
        self.reads
            .request(ReadMsg::ColumnGen { pos, seed, barrier });
    }

    pub fn poll_loaded_column_gen(&self) -> Option<LoadedColumnGen> {
        self.colgen_rx.try_recv().ok()
    }

    pub fn note_colgen_load_miss(&mut self, pos: ChunkPos) {
        self.colgen_manifest.remove(&pos);
    }

    /// Flush everything still queued and join the I/O thread. Call on quit after
    /// sending the final level / entities / chunks: the channel is ordered, so
    /// the join returns only once every prior write has hit disk.
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

/// Open (or create) a world's save directory and spin up its I/O thread.
pub fn open(name: &str) -> std::io::Result<OpenedWorld> {
    open_at(world_dir(name))
}

/// Open (or create) a world at an explicit directory. Backs [`open`]; tests use
/// it directly against a temp dir so they never touch the real data dir.
pub fn open_at(dir: PathBuf) -> std::io::Result<OpenedWorld> {
    let t0 = std::time::Instant::now();
    let region_dir = dir.join("region");
    let explored_dir = dir.join("explored");
    std::fs::create_dir_all(&region_dir)?;
    std::fs::create_dir_all(&explored_dir)?;
    // Refuse a world this build cannot read before anything touches it; back
    // up and restamp an older one (see `format`).
    format::prepare_world(&dir)?;
    // Finish the last save's batch before anything reads the save.
    if journal::recover(&dir)? {
        log::info!("finished an interrupted save in {}", dir.display());
    }
    // A `level.dat` that cannot be read (nor recovered from its backup)
    // refuses the open: a world treated as new would get a fresh seed and
    // world KV written over the real ones.
    let level = level::load(&dir)?;

    // Per-world settings (`settings.json`; absent = defaults). Mod enablement
    // is read BEFORE the palette so disabled-mod content decodes as unknown.
    let world_settings = settings::load(&dir);
    let keep_inventory = world_settings.keep_inventory;
    let day_minutes = world_settings.day_minutes;
    let disabled_mods = world_settings.disabled_mods;

    // Pin (or load) the save's block/item name palette BEFORE any record is
    // read or written: the codec maps every id through it (see `palette`).
    let palette = Arc::new(palette::load_or_create(&dir, &disabled_mods)?);

    // Compare the save's recorded mod set with the ENABLED one (loud warning
    // on any difference; never blocks — a missing mod's content is kept in
    // disk form). Deliberately disabled mods are not a mismatch. A world
    // opened with mods missing is backed up before anything is rewritten.
    let missing_mods: Vec<String> = crate::modding::modset::check_at_open(&dir, &disabled_mods)
        .into_iter()
        .map(|entry| entry.id)
        .collect();
    if !missing_mods.is_empty() {
        format::back_up_small_files(&dir, &missing_mods_backup_name(&missing_mods))?;
    }

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

    let players_dir = dir.join("players");
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
            players_dir,
            dir: world_dir,
            held_writes,
            write_protected: HashSet::new(),
            protected_players: Mutex::new(HashSet::new()),
            kept_sections: Mutex::new(HashMap::new()),
            kept_players: Mutex::new(HashMap::new()),
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
