//! Save-side name↔id palette for blocks and items.
//!
//! Chunk records and item slots store raw `u8` ids. Those ids are only stable
//! while the runtime registries never renumber — which stops being true the
//! moment mod packs (or a future dynamic registry) can add blocks and items.
//! The palette pins a save's ids to NAMES: `palette.json` in the save dir
//! lists, in disk-id order, the block and item names the save was written
//! with. Encode maps runtime ids → the save's disk ids and decode maps back,
//! both through those stable names, so re-numbering the registries can never
//! corrupt an existing world.
//!
//! Rules that keep this sound:
//! - The palette file is APPEND-ONLY: content the save has never seen is
//!   appended (new disk ids); existing lines never move. Old records stay
//!   valid forever.
//! - Disk id 0 must be `air` for both lists — the codec uses `0` as the
//!   empty-slot sentinel — and loading validates that.
//! - A disk name this build doesn't know (a save touched by a newer/modded
//!   build) decodes to air, with a warning, rather than to a wrong block.
//! - A save without the file (created before palettes existed) gets the
//!   IDENTITY palette — correct, because such saves were written with the
//!   current registry order — and the file is written so the save is pinned
//!   from then on.

use std::path::Path;
use std::sync::{Arc, OnceLock, RwLock};

use crate::block::Block;
use crate::item::ItemType;

/// Bidirectional id maps for one save. Both directions are dense 256-entry
/// LUTs, so remapping a section's 4096 block bytes is a table walk.
pub struct Palette {
    block_to_disk: [u8; 256],
    block_from_disk: [u8; 256],
    item_to_disk: [u8; 256],
    item_from_disk: [u8; 256],
}

impl Palette {
    fn identity() -> Palette {
        let mut id = [0u8; 256];
        for (i, v) in id.iter_mut().enumerate() {
            *v = i as u8;
        }
        Palette {
            block_to_disk: id,
            block_from_disk: id,
            item_to_disk: id,
            item_from_disk: id,
        }
    }

    #[inline]
    pub fn block_to_disk(&self, id: u8) -> u8 {
        self.block_to_disk[id as usize]
    }

    #[inline]
    pub fn block_from_disk(&self, id: u8) -> u8 {
        self.block_from_disk[id as usize]
    }

    #[inline]
    pub fn item_to_disk(&self, id: u8) -> u8 {
        self.item_to_disk[id as usize]
    }

    #[inline]
    pub fn item_from_disk(&self, id: u8) -> u8 {
        self.item_from_disk[id as usize]
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PaletteFile {
    blocks: Vec<String>,
    items: Vec<String>,
}

/// A block's stable serde name (`oak_log`) — the palette's identity currency.
fn block_name(b: Block) -> String {
    match serde_json::to_value(b).expect("Block serializes") {
        serde_json::Value::String(s) => s,
        v => unreachable!("Block serialized to non-string {v:?}"),
    }
}

fn block_from_name(name: &str) -> Option<Block> {
    serde_json::from_value(serde_json::Value::String(name.to_owned())).ok()
}

fn item_name(it: ItemType) -> String {
    match serde_json::to_value(it).expect("ItemType serializes") {
        serde_json::Value::String(s) => s,
        v => unreachable!("ItemType serialized to non-string {v:?}"),
    }
}

fn item_from_name(name: &str) -> Option<ItemType> {
    serde_json::from_value(serde_json::Value::String(name.to_owned())).ok()
}

/// Load the save's palette, creating (or extending) `palette.json` as needed.
/// Panics on a corrupt file: guessing at id meanings would silently corrupt
/// the world, so refusing to open is the safe failure.
pub fn load_or_create(dir: &Path) -> std::io::Result<Palette> {
    let path = dir.join("palette.json");
    let (mut file, existed) = match std::fs::read_to_string(&path) {
        Ok(text) => {
            let f: PaletteFile = serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("corrupt save palette {}: {e}", path.display()));
            (f, true)
        }
        Err(_) => (
            // Fresh (or pre-palette) save: pin the current registry order.
            PaletteFile {
                blocks: Block::ALL.iter().map(|&b| block_name(b)).collect(),
                items: ItemType::ALL.iter().map(|&i| item_name(i)).collect(),
            },
            false,
        ),
    };
    if file.blocks.first().map(String::as_str) != Some("air")
        || file.items.first().map(String::as_str) != Some("air")
    {
        panic!(
            "corrupt save palette {}: disk id 0 must be 'air' (the empty-slot sentinel)",
            path.display()
        );
    }

    // Append-only extension: pin any runtime content the save hasn't seen yet.
    let mut changed = !existed;
    for &b in Block::ALL {
        let name = block_name(b);
        if !file.blocks.contains(&name) {
            file.blocks.push(name);
            changed = true;
        }
    }
    for &i in ItemType::ALL {
        let name = item_name(i);
        if !file.items.contains(&name) {
            file.items.push(name);
            changed = true;
        }
    }
    if file.blocks.len() > 256 || file.items.len() > 256 {
        panic!(
            "save palette {} exceeds 256 entries; the record format stores ids in one byte",
            path.display()
        );
    }
    if changed {
        std::fs::write(&path, serde_json::to_string_pretty(&file).expect("serializes"))?;
    }

    // Build the LUTs. Unknown disk names map to air (0); the to-disk side is
    // total after the append above.
    let mut p = Palette {
        block_to_disk: [0; 256],
        block_from_disk: [0; 256],
        item_to_disk: [0; 256],
        item_from_disk: [0; 256],
    };
    for (disk, name) in file.blocks.iter().enumerate() {
        match block_from_name(name) {
            Some(b) => {
                p.block_from_disk[disk] = b.id();
                p.block_to_disk[b.id() as usize] = disk as u8;
            }
            None => log::warn!(
                "save palette: unknown block '{name}' (disk id {disk}) decodes as air — \
                 was this world last played on a newer or modded build?"
            ),
        }
    }
    for (disk, name) in file.items.iter().enumerate() {
        match item_from_name(name) {
            Some(i) => {
                p.item_from_disk[disk] = i.id();
                p.item_to_disk[i.id() as usize] = disk as u8;
            }
            None => log::warn!(
                "save palette: unknown item '{name}' (disk id {disk}) decodes as air"
            ),
        }
    }
    Ok(p)
}

static ACTIVE: RwLock<Option<Arc<Palette>>> = RwLock::new(None);

/// Make `dir`'s palette the one the codec maps through (called when a world
/// opens; the save I/O worker reads it from any thread).
pub fn activate(dir: &Path) -> std::io::Result<()> {
    let p = load_or_create(dir)?;
    *ACTIVE.write().expect("palette lock") = Some(Arc::new(p));
    Ok(())
}

/// The palette codec calls map through: the opened world's, or identity when
/// no world is open (unit tests round-tripping records in isolation).
pub fn active() -> Arc<Palette> {
    if let Some(p) = ACTIVE.read().expect("palette lock").as_ref() {
        return p.clone();
    }
    static IDENTITY: OnceLock<Arc<Palette>> = OnceLock::new();
    IDENTITY.get_or_init(|| Arc::new(Palette::identity())).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("llamacraft-palette-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn fresh_save_gets_identity_palette_and_a_pinned_file() {
        let dir = temp_dir("fresh");
        let p = load_or_create(&dir).unwrap();
        for &b in Block::ALL {
            assert_eq!(p.block_to_disk(b.id()), b.id(), "{b:?} identity");
            assert_eq!(p.block_from_disk(b.id()), b.id(), "{b:?} identity");
        }
        assert!(dir.join("palette.json").exists(), "palette pinned on creation");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shuffled_palette_round_trips_and_remaps() {
        // A palette whose block list is rotated by one relative to the current
        // registry (air stays at 0): to-disk and from-disk must invert each
        // other, and the mapping must actually differ from identity.
        let dir = temp_dir("shuffled");
        let mut blocks: Vec<String> = Block::ALL.iter().map(|&b| block_name(b)).collect();
        blocks[1..].rotate_left(1);
        let items: Vec<String> = ItemType::ALL.iter().map(|&i| item_name(i)).collect();
        let file = PaletteFile { blocks, items };
        std::fs::write(
            dir.join("palette.json"),
            serde_json::to_string(&file).unwrap(),
        )
        .unwrap();
        let p = load_or_create(&dir).unwrap();
        let mut remapped_any = false;
        for &b in Block::ALL {
            let disk = p.block_to_disk(b.id());
            assert_eq!(p.block_from_disk(disk), b.id(), "{b:?} round-trips");
            remapped_any |= disk != b.id();
        }
        assert!(remapped_any, "rotation must produce non-identity ids");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_disk_names_decode_to_air_and_registry_gets_appended() {
        let dir = temp_dir("unknown");
        // A save from "the future": disk id 1 is a block this build lacks.
        let mut blocks = vec!["air".to_string(), "unobtainium".to_string()];
        blocks.extend(Block::ALL.iter().skip(1).map(|&b| block_name(b)));
        let items: Vec<String> = ItemType::ALL.iter().map(|&i| item_name(i)).collect();
        std::fs::write(
            dir.join("palette.json"),
            serde_json::to_string(&PaletteFile { blocks, items }).unwrap(),
        )
        .unwrap();
        let p = load_or_create(&dir).unwrap();
        assert_eq!(p.block_from_disk(1), 0, "unknown disk name decodes to air");
        // Every current block still has a disk id (shifted by the stranger).
        for &b in Block::ALL {
            assert_eq!(p.block_from_disk(p.block_to_disk(b.id())), b.id());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
