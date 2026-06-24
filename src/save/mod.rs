//! Native world saving: a per-world directory under the OS data dir holding a
//! `level.dat` (seed, player, inventory, tick), `entities.dat` (dropped items),
//! and `region/` files packing the chunks the player has modified. Everything
//! else regenerates from the seed, so a save stays small.
//!
//! Disk I/O (compression + file reads/writes) runs on a dedicated thread so the
//! 20 TPS game loop never blocks. The game thread sends snapshots / requests and
//! drains loaded chunks via [`WorldSave::poll_loaded`], mirroring the chunk-gen
//! worker pool.

mod chest;
mod codec;
pub mod entities;
mod furnace;
pub mod level;
pub mod mobs;
mod region;
mod torch;

pub use codec::ChunkSnapshot;
pub use level::LevelData;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::thread::JoinHandle;

use crate::chunk::{Chunk, ChunkPos};
use crate::entity::DroppedItem;
use crate::mob::SavedMob;

/// Messages from the game thread to the I/O thread.
enum IoMsg {
    SaveChunks(Vec<ChunkSnapshot>),
    SaveLevel(Vec<u8>),
    Load(ChunkPos),
    Shutdown,
}

/// A chunk read back from disk (`chunk` is `None` if absent / corrupt) plus any
/// item entities and mobs stored in its record (empty if absent / corrupt / none
/// saved).
pub struct LoadedChunk {
    pub pos: ChunkPos,
    pub chunk: Option<Chunk>,
    pub entities: Vec<DroppedItem>,
    pub mobs: Vec<SavedMob>,
}

/// Live handle to a world's on-disk save and its I/O thread.
pub struct WorldSave {
    tx: Sender<IoMsg>,
    load_rx: Receiver<LoadedChunk>,
    handle: Option<JoinHandle<()>>,
    /// Chunk coords present on disk: seeded at open from region headers, grown
    /// as we save. The load path consults it to choose load-from-disk vs
    /// regenerate.
    manifest: HashSet<ChunkPos>,
    /// Chunk coords whose written record currently carries live entities — dropped
    /// items OR mobs. A chunk leaves the set when re-saved with neither. The persist
    /// decision consults it so a chunk whose drops were picked up / despawned (or whose
    /// mobs wandered off, died, or distance-despawned) is rewritten to clear its
    /// now-stale record, instead of leaving the disk copy to resurrect them on the next
    /// load. Populated both when we save such a record and when we read one back (so
    /// cross-session staleness is seen).
    entities_on_disk: HashSet<ChunkPos>,
}

/// The result of opening (or creating) a world.
pub struct OpenedWorld {
    pub save: WorldSave,
    /// `Some` if a `level.dat` already existed (a returning world).
    pub level: Option<LevelData>,
}

impl WorldSave {
    /// `true` if `pos` has a saved record on disk (or saved this session).
    pub fn manifest_contains(&self, pos: ChunkPos) -> bool {
        self.manifest.contains(&pos)
    }

    /// `true` if `pos`'s written record currently carries live entities (dropped items
    /// or mobs) — so a save that now finds the chunk free of both must rewrite it, or
    /// the stale record resurrects them on the next load.
    pub fn record_holds_entities(&self, pos: ChunkPos) -> bool {
        self.entities_on_disk.contains(&pos)
    }

    /// Note that `pos`'s on-disk record carries live entities (drops or mobs), learned
    /// by reading it back. Mirrors what [`save_chunks`](Self::save_chunks) records when
    /// it writes them, so a record saved in a *previous* session is still rewritten once
    /// its entities are gone.
    pub fn note_record_holds_entities(&mut self, pos: ChunkPos) {
        self.entities_on_disk.insert(pos);
    }

    /// Queue modified chunks for compression + region write (non-blocking).
    pub fn save_chunks(&mut self, snaps: Vec<ChunkSnapshot>) {
        if snaps.is_empty() {
            return;
        }
        for s in &snaps {
            self.manifest.insert(s.pos);
            // Track whether the record we're about to write carries any live entities —
            // drops or mobs (matching `encode_snapshot`'s FLAG_HAS_ENTITIES /
            // FLAG_HAS_MOBS). A chunk that loses them all is then re-saved once to clear
            // the record (see the persist decisions in `world::stream`/`world::store`).
            if s.entities.is_empty() && s.mobs.is_empty() {
                self.entities_on_disk.remove(&s.pos);
            } else {
                self.entities_on_disk.insert(s.pos);
            }
        }
        let _ = self.tx.send(IoMsg::SaveChunks(snaps));
    }

    pub fn save_level(&self, bytes: Vec<u8>) {
        let _ = self.tx.send(IoMsg::SaveLevel(bytes));
    }

    /// Ask the I/O thread to read `pos`; the result arrives via [`poll_loaded`].
    ///
    /// [`poll_loaded`]: Self::poll_loaded
    pub fn request_load(&self, pos: ChunkPos) {
        let _ = self.tx.send(IoMsg::Load(pos));
    }

    pub fn poll_loaded(&self) -> Option<LoadedChunk> {
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

/// Directory for a named world: `<data>/saves/<name>/`.
pub fn world_dir(name: &str) -> PathBuf {
    base_data_dir().join("saves").join(sanitize(name))
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
                        manifest.insert(region::chunk_pos(rx, rz, lidx));
                    }
                }
            }
        }
    }

    let (tx, rx) = std::sync::mpsc::channel::<IoMsg>();
    let (load_tx, load_rx) = std::sync::mpsc::channel::<LoadedChunk>();
    let handle = std::thread::Builder::new()
        .name("llamacraft-save".to_string())
        .spawn(move || io_thread(dir, rx, load_tx))
        .expect("spawn save thread");

    Ok(OpenedWorld {
        save: WorldSave {
            tx,
            load_rx,
            handle: Some(handle),
            manifest,
            entities_on_disk: HashSet::new(),
        },
        level,
    })
}

/// The I/O thread loop: process requests in order, doing compression + file I/O
/// off the game loop. Returns (and so the join completes) on `Shutdown`.
fn io_thread(dir: PathBuf, rx: Receiver<IoMsg>, load_tx: Sender<LoadedChunk>) {
    let region_dir = dir.join("region");
    while let Ok(msg) = rx.recv() {
        match msg {
            IoMsg::SaveChunks(snaps) => write_chunks(&region_dir, snaps),
            IoMsg::SaveLevel(bytes) => {
                let _ = write_atomic(&dir.join("level.dat"), &bytes);
            }
            IoMsg::Load(pos) => {
                let (chunk, entities, mobs) = load_chunk(&region_dir, pos);
                let _ = load_tx.send(LoadedChunk {
                    pos,
                    chunk,
                    entities,
                    mobs,
                });
            }
            IoMsg::Shutdown => break,
        }
    }
}

/// Merge snapshots into their region files (read-modify-write per region).
fn write_chunks(region_dir: &Path, snaps: Vec<ChunkSnapshot>) {
    use std::collections::HashMap;
    let mut by_region: HashMap<(i32, i32), Vec<ChunkSnapshot>> = HashMap::new();
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

fn load_chunk(
    region_dir: &Path,
    pos: ChunkPos,
) -> (Option<Chunk>, Vec<DroppedItem>, Vec<SavedMob>) {
    let decoded = (|| {
        let (rx, rz) = region::region_of(pos);
        let path = region::region_path(region_dir, rx, rz);
        let records = region::read_region(&path).ok()?;
        let blob = records.get(&region::local_index(pos))?;
        codec::decode_chunk(pos.cx, pos.cz, blob)
    })();
    match decoded {
        Some((chunk, entities, mobs)) => (Some(chunk), entities, mobs),
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

    fn load_blocking(save: &WorldSave, pos: ChunkPos) -> Option<LoadedChunk> {
        save.request_load(pos);
        for _ in 0..500 {
            if let Some(l) = save.poll_loaded() {
                return Some(l);
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        None
    }

    /// Full disk round-trip through the I/O thread: write a modified chunk (with a
    /// resting item entity carrying a partly-elapsed lifetime) + level in one
    /// session, reopen in another, and read it all back. Item entities now ride in
    /// the chunk record, so the drop returns when its chunk loads.
    #[test]
    fn save_reopen_roundtrips_chunk_level_entities() {
        let dir = temp_world_dir("roundtrip");
        let pos = ChunkPos::new(5, -9);

        {
            let mut opened = open_at(dir.clone()).expect("open fresh");
            assert!(opened.level.is_none(), "fresh world has no level.dat");
            assert!(!opened.save.manifest_contains(pos));

            let mut chunk = Chunk::new(pos.cx, pos.cz);
            chunk.set_block(3, 64, 7, Block::Stone);
            chunk.set_water(3, 65, 7, Block::Water, 0x12);
            let mut snap = ChunkSnapshot::from_chunk(&chunk);
            let mut drop = DroppedItem::new(
                Vec3::new(80.5, 70.0, -39.5),
                ItemStack::new(ItemType::Dirt, 9),
                1,
            );
            drop.ticks_lived = 2500;
            snap.entities.push(drop);
            opened.save.save_chunks(vec![snap]);

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
                "manifest sees saved chunk"
            );

            let loaded = load_blocking(&opened.save, pos).expect("chunk loads from disk");
            let chunk = loaded.chunk.expect("chunk record decodes");
            assert_eq!(chunk.block_raw(3, 64, 7), Block::Stone.id());
            assert_eq!(chunk.block_raw(3, 65, 7), Block::Water.id());
            assert_eq!(chunk.water_meta(3, 65, 7), 0x12);

            // The item entity comes back with its chunk, lifetime intact.
            assert_eq!(loaded.entities.len(), 1);
            assert_eq!(loaded.entities[0].stack, ItemStack::new(ItemType::Dirt, 9));
            assert_eq!(
                loaded.entities[0].ticks_lived, 2500,
                "remaining lifetime persisted"
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The unload/reload dupe, at the save layer: a chunk record written with a
    /// drop, then re-saved drop-free (the drop was picked up), must not bring the
    /// drop back on reload — and `record_holds_entities` must track the transition.
    #[test]
    fn re_saving_a_drop_free_chunk_clears_its_stale_record() {
        let dir = temp_world_dir("clear-stale-drops");
        let pos = ChunkPos::new(2, -4);

        let mut opened = open_at(dir.clone()).expect("open fresh");

        // Unload-with-item: the record is written carrying one drop.
        let mut chunk = Chunk::new(pos.cx, pos.cz);
        chunk.set_block(1, 64, 1, Block::Stone);
        let mut snap = ChunkSnapshot::from_chunk(&chunk);
        snap.entities.push(DroppedItem::new(
            Vec3::new(33.0, 65.0, -63.0),
            ItemStack::new(ItemType::Dirt, 3),
            1,
        ));
        opened.save.save_chunks(vec![snap]);
        assert!(
            opened.save.record_holds_entities(pos),
            "record now carries a drop"
        );

        let with_item = load_blocking(&opened.save, pos).expect("loads with item");
        assert_eq!(with_item.entities.len(), 1, "drop is present before pickup");

        // Pickup-then-unload: the chunk is re-saved with no drops. The channel is
        // ordered, so this write lands before the load below reads it back.
        let empty = ChunkSnapshot::from_chunk(&chunk); // entities default to empty
        opened.save.save_chunks(vec![empty]);
        assert!(
            !opened.save.record_holds_entities(pos),
            "rewrite cleared the flag"
        );

        let after = load_blocking(&opened.save, pos).expect("loads after pickup");
        assert!(
            after.entities.is_empty(),
            "the stale drop must not resurrect"
        );
        // The chunk's own edits survive the rewrite (only the drop was cleared).
        assert_eq!(
            after.chunk.expect("chunk decodes").block_raw(1, 64, 1),
            Block::Stone.id()
        );

        opened.save.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The same stale-record guard, for mobs: a chunk record written with a mob, then
    /// re-saved mob-free (the mob died, wandered off, or distance-despawned), must not
    /// bring the mob back on reload — and `record_holds_entities` must track it. The
    /// guard is one mechanism shared with dropped items, so this pins it for the mob
    /// path too.
    #[test]
    fn re_saving_a_mob_free_chunk_clears_its_stale_record() {
        let dir = temp_world_dir("clear-stale-mobs");
        let pos = ChunkPos::new(-7, 3);

        let mut opened = open_at(dir.clone()).expect("open fresh");

        // Unload-with-mob: the record is written carrying one mob.
        let chunk = Chunk::new(pos.cx, pos.cz);
        let mut snap = ChunkSnapshot::from_chunk(&chunk);
        snap.mobs.push(crate::mob::SavedMob {
            kind: crate::mob::Mob::Owl,
            pos: Vec3::new(-100.5, 65.0, 56.5),
            yaw: 0.5,
        });
        opened.save.save_chunks(vec![snap]);
        assert!(
            opened.save.record_holds_entities(pos),
            "record now carries a mob"
        );

        let with_mob = load_blocking(&opened.save, pos).expect("loads with mob");
        assert_eq!(with_mob.mobs.len(), 1, "mob present before it leaves");

        // The mob is gone: the chunk is re-saved mob-free. The record must be rewritten
        // so the stale mob can't resurrect on the next load.
        let empty = ChunkSnapshot::from_chunk(&chunk);
        opened.save.save_chunks(vec![empty]);
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
