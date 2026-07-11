//! Save-side name↔id palette for blocks, items, and mobs.
//!
//! Chunk records, item slots, and mob records store raw `u8` ids. Those ids are
//! only stable while the runtime registries never renumber — which stops being
//! true the moment mod packs (or a future dynamic registry) can add content.
//! The palette pins a save's ids to NAMES: `palette.json` in the save dir
//! lists, in disk-id order, the block/item/mob names the save was written
//! with. Encode maps runtime ids → the save's disk ids and decode maps back,
//! both through those stable names, so re-numbering the registries can never
//! corrupt an existing world.
//!
//! Rules that keep this sound:
//! - The palette file is APPEND-ONLY: content the save has never seen is
//!   appended (new disk ids); existing lines never move. Old records stay
//!   valid forever.
//! - Disk id 0 must be `air` for the block and item lists — the codec uses `0`
//!   as the empty-slot sentinel — and loading validates that. Mobs have no
//!   such sentinel.
//! - A disk name this build doesn't know (a save touched by a newer/modded
//!   build) decodes to air, with a warning, rather than to a wrong block. An
//!   unknown MOB name has no safe stand-in, so it decodes to `None` and the
//!   record reader skips that mob (see `save::mobs`).
//! - A save without the file (created before palettes existed) gets the
//!   IDENTITY palette — correct, because such saves were written with the
//!   current registry order — and the file is written so the save is pinned
//!   from then on. A pre-mob palette file (no `mobs` list) backfills the same
//!   way: identity from the registry.
//! - Per-world DISABLED mods (`settings.json`): a name namespaced to a
//!   disabled mod id is treated exactly like an unknown name (blocks/items
//!   decode to air/empty, mobs are skipped) even though the registry knows
//!   it, and no NEW palette entries are appended for such names while the mod
//!   is disabled. Entries the file already has STAY (append-only), so
//!   re-enabling the mod restores its world content on the next load — for
//!   records not re-saved while it was disabled (a section saved with the
//!   content decoded to air persists the air).

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Arc, OnceLock, RwLock};

use crate::block::Block;
use crate::item::ItemType;
use crate::mob::Mob;

/// Bidirectional id maps for one save. Both directions are dense 256-entry
/// LUTs, so remapping a section's 4096 block bytes is a table walk.
pub struct Palette {
    block_to_disk: [u8; 256],
    block_from_disk: [u8; 256],
    item_to_disk: [u8; 256],
    item_from_disk: [u8; 256],
    /// `None` = a runtime species this palette has no disk pin for (a
    /// per-world DISABLED mod's species): such a mob cannot be persisted —
    /// there is no air-mob sentinel to write — so the encoder skips it.
    mob_to_disk: [Option<u8>; 256],
    /// `None` = a disk id whose name this build doesn't know (skip the mob).
    mob_from_disk: [Option<u8>; 256],
}

impl Palette {
    fn identity() -> Palette {
        let mut id = [0u8; 256];
        for (i, v) in id.iter_mut().enumerate() {
            *v = i as u8;
        }
        let mut mob_identity = [None; 256];
        for (i, v) in mob_identity.iter_mut().enumerate() {
            *v = Some(i as u8);
        }
        Palette {
            block_to_disk: id,
            block_from_disk: id,
            item_to_disk: id,
            item_from_disk: id,
            mob_to_disk: mob_identity,
            mob_from_disk: mob_identity,
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

    /// The disk id for a runtime mob id, or `None` when this palette carries
    /// no pin for the species (its owning mod is disabled for this world) —
    /// the encoder must SKIP such a mob (no air-mob sentinel exists).
    #[inline]
    pub fn mob_to_disk(&self, id: u8) -> Option<u8> {
        self.mob_to_disk[id as usize]
    }

    /// The runtime mob id for a disk id, or `None` when the save's name for it is
    /// unknown to this build — the reader must SKIP such a mob (there is no "air
    /// mob" to degrade to, and guessing a species would corrupt the world).
    #[inline]
    pub fn mob_from_disk(&self, id: u8) -> Option<u8> {
        self.mob_from_disk[id as usize]
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PaletteFile {
    blocks: Vec<String>,
    items: Vec<String>,
    /// Absent in palettes written before mobs were palette-pinned; backfilled
    /// as identity from the registry on load.
    #[serde(default)]
    mobs: Vec<String>,
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

fn mob_name(m: Mob) -> String {
    match serde_json::to_value(m).expect("Mob serializes") {
        serde_json::Value::String(s) => s,
        v => unreachable!("Mob serialized to non-string {v:?}"),
    }
}

fn mob_from_name(name: &str) -> Option<Mob> {
    serde_json::from_value(serde_json::Value::String(name.to_owned())).ok()
}

/// Whether `name` belongs to a mod id in `disabled`. The engine `petramond`
/// namespace is reserved and never appears in the disabled mod-id set.
fn name_disabled(name: &str, disabled: &BTreeSet<String>) -> bool {
    crate::registry::namespace(name).is_some_and(|ns| disabled.contains(ns))
}

/// Load the save's palette, creating (or extending) `palette.json` as needed.
/// Content namespaced to a mod id in `disabled` is treated as unknown and not
/// appended (see the module docs). Panics on a corrupt file: guessing at id
/// meanings would silently corrupt the world, so refusing to open is the safe
/// failure.
pub fn load_or_create(dir: &Path, disabled: &BTreeSet<String>) -> std::io::Result<Palette> {
    let path = dir.join("palette.json");
    let (mut file, existed) = match std::fs::read_to_string(&path) {
        Ok(text) => {
            let f: PaletteFile = serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("corrupt save palette {}: {e}", path.display()));
            (f, true)
        }
        // Fresh (or pre-palette) save: the append below pins the current
        // registry order (air first — the engine lists lead with it).
        Err(_) => (
            PaletteFile {
                blocks: Vec::new(),
                items: Vec::new(),
                mobs: Vec::new(),
            },
            false,
        ),
    };

    // Append-only extension: pin any runtime content the save hasn't seen
    // yet, EXCEPT names owned by a disabled mod (no new entries while
    // disabled; existing entries stay — re-enabling restores them).
    let mut changed = !existed;
    for &b in Block::all() {
        let name = block_name(b);
        if !file.blocks.contains(&name) && !name_disabled(&name, disabled) {
            file.blocks.push(name);
            changed = true;
        }
    }
    for &i in ItemType::all() {
        let name = item_name(i);
        if !file.items.contains(&name) && !name_disabled(&name, disabled) {
            file.items.push(name);
            changed = true;
        }
    }
    // A pre-mob palette has an empty `mobs` list; this same append backfills it
    // as identity (registry order), which is what such saves were written with.
    for &m in Mob::all() {
        let name = mob_name(m);
        if !file.mobs.contains(&name) && !name_disabled(&name, disabled) {
            file.mobs.push(name);
            changed = true;
        }
    }
    if file.blocks.first().map(String::as_str) != Some("petramond:air")
        || file.items.first().map(String::as_str) != Some("petramond:air")
    {
        panic!(
            "corrupt save palette {}: disk id 0 must be 'petramond:air' (the empty-slot sentinel)",
            path.display()
        );
    }
    if file.blocks.len() > 256 || file.items.len() > 256 || file.mobs.len() > 256 {
        panic!(
            "save palette {} exceeds 256 entries; the record format stores ids in one byte",
            path.display()
        );
    }
    if changed {
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&file).expect("serializes"),
        )?;
    }

    // Build the LUTs. Unknown disk names map to air (0) for blocks/items and
    // to a skip (`None`) for mobs. Names owned by a DISABLED mod get the same
    // treatment even though the registry knows them: their world content
    // vanishes for this session, and nothing at runtime can encode to their
    // disk ids. The to-disk side is total after the append above, except for
    // disabled species (mob_to_disk = None → the encoder skips the mob).
    let mut p = Palette {
        block_to_disk: [0; 256],
        block_from_disk: [0; 256],
        item_to_disk: [0; 256],
        item_from_disk: [0; 256],
        mob_to_disk: [None; 256],
        mob_from_disk: [None; 256],
    };
    for (disk, name) in file.blocks.iter().enumerate() {
        match block_from_name(name) {
            Some(_) if name_disabled(name, disabled) => log::info!(
                "save palette: block '{name}' (disk id {disk}) decodes as air — its mod is \
                 disabled for this world"
            ),
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
            Some(_) if name_disabled(name, disabled) => log::info!(
                "save palette: item '{name}' (disk id {disk}) decodes as empty — its mod is \
                 disabled for this world"
            ),
            Some(i) => {
                p.item_from_disk[disk] = i.id();
                p.item_to_disk[i.id() as usize] = disk as u8;
            }
            None => {
                log::warn!("save palette: unknown item '{name}' (disk id {disk}) decodes as air")
            }
        }
    }
    for (disk, name) in file.mobs.iter().enumerate() {
        match mob_from_name(name) {
            Some(_) if name_disabled(name, disabled) => log::info!(
                "save palette: mob '{name}' (disk id {disk}) is skipped on load — its mod is \
                 disabled for this world"
            ),
            Some(m) => {
                p.mob_from_disk[disk] = Some(m.id());
                p.mob_to_disk[m.id() as usize] = Some(disk as u8);
            }
            None => log::warn!(
                "save palette: unknown mob '{name}' (disk id {disk}); such mobs are \
                 skipped on load (never respawned as a wrong species)"
            ),
        }
    }
    Ok(p)
}

static ACTIVE: RwLock<Option<Arc<Palette>>> = RwLock::new(None);

/// Make `dir`'s palette the one the codec maps through (called when a world
/// opens; the save I/O worker reads it from any thread). `disabled` = the
/// world's disabled mod ids (`settings.json`).
pub fn activate(dir: &Path, disabled: &BTreeSet<String>) -> std::io::Result<()> {
    let p = load_or_create(dir, disabled)?;
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
    IDENTITY
        .get_or_init(|| Arc::new(Palette::identity()))
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("petramond-palette-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn no_disabled() -> BTreeSet<String> {
        BTreeSet::new()
    }

    #[test]
    fn fresh_save_gets_identity_palette_and_a_pinned_file() {
        let dir = temp_dir("fresh");
        let p = load_or_create(&dir, &no_disabled()).unwrap();
        for &b in Block::all() {
            assert_eq!(p.block_to_disk(b.id()), b.id(), "{b:?} identity");
            assert_eq!(p.block_from_disk(b.id()), b.id(), "{b:?} identity");
        }
        assert!(
            dir.join("palette.json").exists(),
            "palette pinned on creation"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shuffled_palette_round_trips_and_remaps() {
        // A palette whose block list is rotated by one relative to the current
        // registry (air stays at 0): to-disk and from-disk must invert each
        // other, and the mapping must actually differ from identity.
        let dir = temp_dir("shuffled");
        let mut blocks: Vec<String> = Block::all().iter().map(|&b| block_name(b)).collect();
        blocks[1..].rotate_left(1);
        let items: Vec<String> = ItemType::all().iter().map(|&i| item_name(i)).collect();
        let mobs: Vec<String> = Mob::all().iter().map(|&m| mob_name(m)).collect();
        let file = PaletteFile {
            blocks,
            items,
            mobs,
        };
        std::fs::write(
            dir.join("palette.json"),
            serde_json::to_string(&file).unwrap(),
        )
        .unwrap();
        let p = load_or_create(&dir, &no_disabled()).unwrap();
        let mut remapped_any = false;
        for &b in Block::all() {
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
        let mut blocks = vec!["petramond:air".to_string(), "unobtainium".to_string()];
        blocks.extend(Block::all().iter().skip(1).map(|&b| block_name(b)));
        let items: Vec<String> = ItemType::all().iter().map(|&i| item_name(i)).collect();
        let mobs: Vec<String> = Mob::all().iter().map(|&m| mob_name(m)).collect();
        std::fs::write(
            dir.join("palette.json"),
            serde_json::to_string(&PaletteFile {
                blocks,
                items,
                mobs,
            })
            .unwrap(),
        )
        .unwrap();
        let p = load_or_create(&dir, &no_disabled()).unwrap();
        assert_eq!(p.block_from_disk(1), 0, "unknown disk name decodes to air");
        // Every current block still has a disk id (shifted by the stranger).
        for &b in Block::all() {
            assert_eq!(p.block_from_disk(p.block_to_disk(b.id())), b.id());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn pre_mob_palette_backfills_identity_and_pins_the_list() {
        // A palette written before mobs were pinned: no `mobs` list at all. It
        // must load (serde default), backfill identity from the registry, and
        // rewrite the file with the list pinned.
        let dir = temp_dir("premob");
        let blocks: Vec<String> = Block::all().iter().map(|&b| block_name(b)).collect();
        let items: Vec<String> = ItemType::all().iter().map(|&i| item_name(i)).collect();
        std::fs::write(
            dir.join("palette.json"),
            serde_json::json!({ "blocks": blocks, "items": items }).to_string(),
        )
        .unwrap();
        let p = load_or_create(&dir, &no_disabled()).unwrap();
        for &m in Mob::all() {
            assert_eq!(p.mob_to_disk(m.id()), Some(m.id()), "{m:?} identity");
            assert_eq!(p.mob_from_disk(m.id()), Some(m.id()), "{m:?} identity");
        }
        let text = std::fs::read_to_string(dir.join("palette.json")).unwrap();
        assert!(text.contains("\"mobs\""), "the mob list is pinned on load");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_mob_names_decode_to_a_skip_and_known_ones_remap() {
        // A save whose mob list starts with a species this build lacks: known
        // mobs remap by name around it; the stranger's disk id decodes to None
        // (the record reader skips such mobs — there is no air mob).
        let dir = temp_dir("mobstranger");
        let blocks: Vec<String> = Block::all().iter().map(|&b| block_name(b)).collect();
        let items: Vec<String> = ItemType::all().iter().map(|&i| item_name(i)).collect();
        let mut mobs = vec!["othermod:phantom".to_string()];
        mobs.extend(Mob::all().iter().map(|&m| mob_name(m)));
        std::fs::write(
            dir.join("palette.json"),
            serde_json::to_string(&PaletteFile {
                blocks,
                items,
                mobs,
            })
            .unwrap(),
        )
        .unwrap();
        let p = load_or_create(&dir, &no_disabled()).unwrap();
        assert_eq!(p.mob_from_disk(0), None, "the stranger decodes to a skip");
        let mut remapped_any = false;
        for &m in Mob::all() {
            let disk = p
                .mob_to_disk(m.id())
                .expect("every enabled species has a disk pin");
            assert_eq!(p.mob_from_disk(disk), Some(m.id()), "{m:?} round-trips");
            remapped_any |= disk != m.id();
        }
        assert!(remapped_any, "the stranger shifts every known disk id");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The per-world disabled-mod palette contract (per-world
    /// mods): while a mod is disabled its namespaced names get NO new palette
    /// entries and existing entries decode as unknown (blocks→air, items→
    /// empty, no to-disk pin); re-enabling restores the mapping from the
    /// untouched append-only file. Needs a registered dynamic name, so it runs
    /// in a child process with a fixture pack (the 2a `PETRAMOND_MODS`
    /// re-spawn pattern).
    #[test]
    fn disabled_mod_content_gets_the_unknown_treatment_and_reenabling_restores() {
        let root = std::env::temp_dir().join(format!("petramond-paldis-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let pack = root.join("mods/palmod");
        std::fs::create_dir_all(&pack).unwrap();
        std::fs::write(
            pack.join("pack.json"),
            r#"{ "name": "Palette Mod", "id": "palmod" }"#,
        )
        .unwrap();
        std::fs::write(
            pack.join("blocks.json"),
            r#"{ "blocks": [ { "block": "palmod:relic", "shape": "cube", "flags": ["solid", "opaque", "ao_occluder"], "tags": [], "behavior": "inert", "interaction": "none", "collision": [{"min": [0, 0, 0], "max": [1, 1, 1]}], "emission": 0, "tiles": ["stone", "stone", "stone"], "material": "stone", "harvest_tier": 1, "hardness": 2, "drops": [{"item": "palmod:relic", "min": 1, "max": 1, "chance": 1.0}] } ] }"#,
        )
        .unwrap();
        std::fs::write(
            pack.join("items.json"),
            r#"{ "items": [ { "item": "palmod:relic", "key": "palmod:relic", "name": "Relic", "max_stack_size": 64, "held_pose": {"pitch": 0, "yaw": 1.8, "roll": 0}, "tags": [], "block": "palmod:relic" } ] }"#,
        )
        .unwrap();

        let exe = std::env::current_exe().expect("test binary path");
        let out = std::process::Command::new(exe)
            .arg("save::palette::tests::disabled_mod_palette_inner")
            .arg("--exact")
            .arg("--ignored")
            .arg("--nocapture")
            .env("PETRAMOND_MODS", root.join("mods"))
            .env("PETRAMOND_PALDIS_SAVE", root.join("save"))
            .output()
            .expect("spawn test binary");
        let _ = std::fs::remove_dir_all(&root);
        assert!(
            out.status.success(),
            "inner test failed\n--- stdout ---\n{}\n--- stderr ---\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
    }

    /// Runs ONLY in the child process spawned above (needs `PETRAMOND_MODS`
    /// pointing at the fixture pack before first registry touch).
    #[test]
    #[ignore = "spawned by disabled_mod_content_gets_the_unknown_treatment_and_reenabling_restores"]
    fn disabled_mod_palette_inner() {
        let save = std::path::PathBuf::from(std::env::var_os("PETRAMOND_PALDIS_SAVE").unwrap());
        std::fs::create_dir_all(&save).unwrap();
        let disabled: BTreeSet<String> = ["palmod".to_owned()].into();

        let relic = block_from_name("palmod:relic").expect("fixture block registered");
        let relic_item = item_from_name("palmod:relic").expect("fixture item registered");

        // Fresh save opened with the mod DISABLED: no palette entry appended,
        // and the runtime id has no disk pin (encodes as air/empty).
        let p = load_or_create(&save, &disabled).unwrap();
        let text = std::fs::read_to_string(save.join("palette.json")).unwrap();
        assert!(
            !text.contains("palmod:relic"),
            "no new palette entries while disabled"
        );
        assert_eq!(
            p.block_to_disk(relic.id()),
            0,
            "encodes as air while unpinned"
        );
        assert_eq!(p.item_to_disk(relic_item.id()), 0);

        // The mod enabled: the entry appends and round-trips.
        let p = load_or_create(&save, &BTreeSet::new()).unwrap();
        let disk = p.block_to_disk(relic.id());
        assert_ne!(disk, 0, "enabled content gets a real disk id");
        assert_eq!(p.block_from_disk(disk), relic.id());
        let item_disk = p.item_to_disk(relic_item.id());
        assert_eq!(p.item_from_disk(item_disk), relic_item.id());

        // Disabled again, entry NOW IN THE FILE: decodes as unknown (air /
        // empty), no to-disk pin, and the entry itself stays (append-only).
        let p = load_or_create(&save, &disabled).unwrap();
        assert_eq!(p.block_from_disk(disk), 0, "disabled block decodes to air");
        assert_eq!(
            p.item_from_disk(item_disk),
            0,
            "disabled item decodes to empty"
        );
        assert_eq!(p.block_to_disk(relic.id()), 0);
        let text = std::fs::read_to_string(save.join("palette.json")).unwrap();
        assert!(
            text.contains("palmod:relic"),
            "existing entries stay in the file while disabled"
        );

        // Re-enabled: the untouched entry restores the exact mapping.
        let p = load_or_create(&save, &BTreeSet::new()).unwrap();
        assert_eq!(p.block_from_disk(disk), relic.id(), "re-enabling restores");
        assert_eq!(p.block_to_disk(relic.id()), disk, "same disk id as before");
        assert_eq!(p.item_from_disk(item_disk), relic_item.id());
    }
}
