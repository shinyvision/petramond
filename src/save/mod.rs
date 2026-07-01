//! Native world saving: a per-world directory under the OS data dir holding a
//! `level.dat` (seed, player, inventory, tick) and `region/` files packing the
//! 16³ sections the player has modified. Everything else regenerates from the
//! seed, so a save stays small.
//!
//! Disk I/O (compression + file reads/writes) runs on a dedicated thread so the
//! 20 TPS game loop never blocks. The game thread sends snapshots / requests and
//! drains loaded sections via [`WorldSave::poll_loaded`], mirroring the section-gen
//! worker pool.

mod chest;
mod codec;
pub mod entities;
mod furnace;
pub mod level;
pub mod mobs;
mod region;
mod torch;

pub use codec::SectionSnapshot;
pub use level::LevelData;

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::thread::JoinHandle;

use crate::chunk::{ChunkPos, SectionPos};
use crate::entity::DroppedItem;
use crate::mob::SavedMob;
use crate::section::Section;

/// Messages from the game thread to the I/O thread.
enum IoMsg {
    SaveSections(Vec<SectionSnapshot>),
    SaveLevel(Vec<u8>),
    Load(SectionPos),
    Shutdown,
}

/// A section read back from disk (`section` is `None` if absent / corrupt) plus any
/// item entities and mobs stored in its record (empty if absent / corrupt / none
/// saved).
pub struct LoadedSection {
    pub pos: SectionPos,
    pub section: Option<Section>,
    pub entities: Vec<DroppedItem>,
    pub mobs: Vec<SavedMob>,
}

/// Live handle to a world's on-disk save and its I/O thread.
pub struct WorldSave {
    tx: Sender<IoMsg>,
    load_rx: Receiver<LoadedSection>,
    handle: Option<JoinHandle<()>>,
    /// Section coords present on disk: seeded at open from region headers, grown
    /// as we save. The load path consults it to choose overlay-from-disk vs
    /// regenerate.
    manifest: HashSet<SectionPos>,
    /// Per-column view of `manifest`, so the streamer's per-column wanted-section
    /// scans don't walk the whole manifest for every column (that made vertical
    /// crossings O(columns × manifest) on a lived-in save).
    manifest_columns: HashMap<ChunkPos, Vec<i32>>,
    /// Section coords whose written record currently carries live entities — dropped
    /// items OR mobs. A section leaves the set when re-saved with neither. The persist
    /// decision consults it so a section whose drops were picked up / despawned (or whose
    /// mobs wandered off, died, or distance-despawned) is rewritten to clear its
    /// now-stale record, instead of leaving the disk copy to resurrect them on the next
    /// load. Populated both when we save such a record and when we read one back (so
    /// cross-session staleness is seen).
    entities_on_disk: HashSet<SectionPos>,
}

/// The result of opening (or creating) a world.
pub struct OpenedWorld {
    pub save: WorldSave,
    /// `Some` if a `level.dat` already existed (a returning world).
    pub level: Option<LevelData>,
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
    /// `true` if `pos` has a saved record on disk (or saved this session).
    pub fn manifest_contains(&self, pos: SectionPos) -> bool {
        self.manifest.contains(&pos)
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
        for s in &snaps {
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
        }
        let _ = self.tx.send(IoMsg::SaveSections(snaps));
    }

    pub fn save_level(&self, bytes: Vec<u8>) {
        let _ = self.tx.send(IoMsg::SaveLevel(bytes));
    }

    /// Ask the I/O thread to read `pos`; the result arrives via [`poll_loaded`].
    ///
    /// [`poll_loaded`]: Self::poll_loaded
    pub fn request_load(&self, pos: SectionPos) {
        let _ = self.tx.send(IoMsg::Load(pos));
    }

    pub fn poll_loaded(&self) -> Option<LoadedSection> {
        self.load_rx.try_recv().ok()
    }

    /// Flush everything still queued and join the I/O thread. Call on quit after
    /// sending the final level / entities / chunks: the channel is ordered, so
    /// the join returns only once every prior write has hit disk.
    pub fn shutdown(&mut self) {
        let _ = self.tx.send(IoMsg::Shutdown);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for WorldSave {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Base data dir: `~/.local/share/llamacraft` (Linux), `~/Library/Application
/// Support/llamacraft` (macOS), `%APPDATA%\llamacraft` (Windows). Falls back to
/// a hidden dir in the cwd if no home dir can be resolved.
fn base_data_dir() -> PathBuf {
    directories::ProjectDirs::from("", "", "llamacraft")
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".llamacraft"))
}

fn saves_dir() -> PathBuf {
    base_data_dir().join("saves")
}

/// Directory for a named world: `<data>/saves/<name>/`.
pub fn world_dir(name: &str) -> PathBuf {
    saves_dir().join(sanitize(name))
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

pub fn delete_world(dir_name: &str) -> std::io::Result<()> {
    delete_world_at(&saves_dir(), dir_name)
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
    std::fs::create_dir_all(&region_dir)?;

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

    let (tx, rx) = std::sync::mpsc::channel::<IoMsg>();
    let (load_tx, load_rx) = std::sync::mpsc::channel::<LoadedSection>();
    let handle = std::thread::Builder::new()
        .name("llamacraft-save".to_string())
        .spawn(move || io_thread(dir, rx, load_tx))
        .expect("spawn save thread");

    let mut manifest_columns: HashMap<ChunkPos, Vec<i32>> = HashMap::new();
    for sp in &manifest {
        manifest_columns.entry(sp.chunk_pos()).or_default().push(sp.cy);
    }

    Ok(OpenedWorld {
        save: WorldSave {
            tx,
            load_rx,
            handle: Some(handle),
            manifest,
            manifest_columns,
            entities_on_disk: HashSet::new(),
        },
        level,
    })
}

/// The I/O thread loop: process requests in order, doing compression + file I/O
/// off the game loop. Returns (and so the join completes) on `Shutdown`.
fn io_thread(dir: PathBuf, rx: Receiver<IoMsg>, load_tx: Sender<LoadedSection>) {
    let region_dir = dir.join("region");
    while let Ok(msg) = rx.recv() {
        match msg {
            IoMsg::SaveSections(snaps) => write_sections(&region_dir, snaps),
            IoMsg::SaveLevel(bytes) => {
                let _ = write_atomic(&dir.join("level.dat"), &bytes);
            }
            IoMsg::Load(pos) => {
                let (section, entities, mobs) = load_section(&region_dir, pos);
                let _ = load_tx.send(LoadedSection {
                    pos,
                    section,
                    entities,
                    mobs,
                });
            }
            IoMsg::Shutdown => break,
        }
    }
}

/// Merge snapshots into their region files (read-modify-write per region).
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
        // On a corrupt region we start fresh rather than refuse to save; the new
        // records still land, only the unreadable old ones are lost.
        let mut records = region::read_region(&path).unwrap_or_default();
        for s in &group {
            records.insert(region::local_index(s.pos), codec::encode_snapshot(s));
        }
        let _ = region::write_region(&path, &records);
    }
}

fn load_section(
    region_dir: &Path,
    pos: SectionPos,
) -> (Option<Section>, Vec<DroppedItem>, Vec<SavedMob>) {
    let decoded = (|| {
        let (rx, rz) = region::region_of(pos);
        let path = region::region_path(region_dir, rx, rz);
        let records = region::read_region(&path).ok()?;
        let blob = records.get(&region::local_index(pos))?;
        codec::decode_section(pos, blob)
    })();
    match decoded {
        Some((section, entities, mobs)) => (Some(section), entities, mobs),
        None => (None, Vec::new(), Vec::new()),
    }
}

/// Atomic file write: tmp + rename, so a crash mid-write can't truncate the live file.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
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
            std::env::temp_dir().join(format!("llamacraft-savetest-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn load_blocking(save: &WorldSave, pos: SectionPos) -> Option<LoadedSection> {
        save.request_load(pos);
        for _ in 0..500 {
            if let Some(l) = save.poll_loaded() {
                return Some(l);
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        None
    }

    /// Full disk round-trip through the I/O thread: write a modified section (with a
    /// resting item entity carrying a partly-elapsed lifetime) + level in one
    /// session, reopen in another, and read it all back. Item entities ride in the
    /// section record, so the drop returns when its section loads.
    #[test]
    fn save_reopen_roundtrips_section_level_entities() {
        let dir = temp_world_dir("roundtrip");
        let pos = SectionPos::new(5, -3, -9); // negative cy: below the old datum

        {
            let mut opened = open_at(dir.clone()).expect("open fresh");
            assert!(opened.level.is_none(), "fresh world has no level.dat");
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

            let mut player = Player::new(Vec3::new(80.0, 70.0, -40.0));
            player.inventory.set_active(4);
            opened.save.save_level(level::encode(0xABCD, &player, 0));

            opened.save.shutdown(); // flush queued writes + join the I/O thread
        }

        {
            let opened = open_at(dir.clone()).expect("reopen");

            let level = opened.level.expect("level.dat restored");
            assert_eq!(level.seed, 0xABCD);
            assert_eq!(level.player_pos, Vec3::new(80.0, 70.0, -40.0));
            assert_eq!(level.inventory.active_slot(), 4);

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
    fn seed_text_accepts_numbers_and_hashes_strings() {
        assert_eq!(seed_from_text("12345"), 12345);
        assert_eq!(seed_from_text(" 12345 "), 12345);
        assert_eq!(
            seed_from_text("llamacraft"),
            seed_from_text("llamacraft"),
            "string seeds are stable"
        );
        assert_ne!(
            seed_from_text("llamacraft"),
            seed_from_text("Llamacraft"),
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
