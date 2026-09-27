use std::collections::BTreeSet;
use std::path::Path;

use crate::mob::Mob;
use petramond_world::block::Block;
use petramond_world::item::ItemType;

pub struct Palette {
    block_to_disk: Box<[u16]>,
    block_from_disk: Box<[u16]>,
    item_to_disk: Box<[u16]>,
    item_from_disk: Box<[u16]>,
    mob_to_disk: [Option<u8>; 256],
    mob_from_disk: [Option<u8>; 256],
}

const UNKNOWN: u16 = u16::MAX;

#[inline]
fn lut(table: &[u16], id: u16) -> u16 {
    table.get(id as usize).copied().unwrap_or(0)
}

#[inline]
fn from_lut(table: &[u16], id: u16) -> Option<u16> {
    match table.get(id as usize) {
        Some(&UNKNOWN) | None => None,
        Some(&runtime) => Some(runtime),
    }
}

impl Palette {
    pub fn identity() -> Palette {
        let ids = |n: usize| -> Box<[u16]> { (0..n as u16).collect::<Vec<_>>().into_boxed_slice() };
        let n = petramond_world::registry::WIDE_ID_CAP;
        let mut mob_identity = [None; 256];
        for (i, v) in mob_identity.iter_mut().enumerate() {
            *v = Some(i as u8);
        }
        Palette {
            block_to_disk: ids(n),
            block_from_disk: ids(n),
            item_to_disk: ids(n),
            item_from_disk: ids(n),
            mob_to_disk: mob_identity,
            mob_from_disk: mob_identity,
        }
    }

    #[inline]
    pub fn block_to_disk(&self, id: u16) -> u16 {
        lut(&self.block_to_disk, id)
    }

    #[inline]
    pub fn block_from_disk(&self, id: u16) -> u16 {
        self.block_from_disk_known(id).unwrap_or(0)
    }

    #[inline]
    pub fn block_from_disk_known(&self, id: u16) -> Option<u16> {
        from_lut(&self.block_from_disk, id)
    }

    #[inline]
    pub fn item_to_disk(&self, id: u16) -> u16 {
        lut(&self.item_to_disk, id)
    }

    #[inline]
    pub fn item_from_disk(&self, id: u16) -> u16 {
        self.item_from_disk_known(id).unwrap_or(0)
    }

    #[inline]
    pub fn item_from_disk_known(&self, id: u16) -> Option<u16> {
        from_lut(&self.item_from_disk, id)
    }

    #[inline]
    pub fn mob_to_disk(&self, id: u8) -> Option<u8> {
        self.mob_to_disk[id as usize]
    }

    #[inline]
    pub fn mob_from_disk(&self, id: u8) -> Option<u8> {
        self.mob_from_disk[id as usize]
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PaletteFile {
    blocks: Vec<String>,
    items: Vec<String>,
    #[serde(default)]
    mobs: Vec<String>,
}

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

fn name_disabled(name: &str, disabled: &BTreeSet<String>) -> bool {
    petramond_world::registry::namespace(name).is_some_and(|ns| disabled.contains(ns))
}

fn owners_past(names: &[String], ceiling: usize) -> String {
    let mut owners: Vec<&str> = Vec::new();
    for name in &names[ceiling.min(names.len())..] {
        let owner = petramond_world::registry::namespace(name).unwrap_or(name.as_str());
        if !owners.contains(&owner) {
            owners.push(owner);
        }
    }
    owners.join(", ")
}

pub fn load_or_create(dir: &Path, disabled: &BTreeSet<String>) -> std::io::Result<Palette> {
    let path = dir.join("palette.json");
    let invalid = |msg: String| std::io::Error::new(std::io::ErrorKind::InvalidData, msg);
    let (mut file, existed) = match std::fs::read_to_string(&path) {
        Ok(text) => {
            let f: PaletteFile = serde_json::from_str(&text)
                .map_err(|e| invalid(format!("corrupt save palette {}: {e}", path.display())))?;
            (f, true)
        }
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
        Err(_) => (
            PaletteFile {
                blocks: Vec::new(),
                items: Vec::new(),
                mobs: Vec::new(),
            },
            false,
        ),
    };

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
        return Err(invalid(format!(
            "corrupt save palette {}: disk id 0 must be 'petramond:air' (the empty-slot sentinel)",
            path.display()
        )));
    }
    let cap = petramond_world::registry::WIDE_ID_CAP;
    for (kind, names, ceiling) in [
        ("block", &file.blocks, cap),
        ("item", &file.items, cap),
        ("mob", &file.mobs, 256),
    ] {
        if names.len() > ceiling {
            return Err(invalid(format!(
                "save palette {} holds {} {kind} kinds, past the ceiling of {ceiling}; the \
                 kinds that do not fit come from: {} (disable some of those mods for this world)",
                path.display(),
                names.len(),
                owners_past(names, ceiling)
            )));
        }
    }
    if changed {
        petramond_persist::atomic_file::replace(
            &path,
            serde_json::to_string_pretty(&file)
                .expect("serializes")
                .as_bytes(),
        )?;
    }

    // Build the LUTs. Unknown disk names stay unresolved (`UNKNOWN` for
    // blocks/items, `None` for mobs): the codecs keep their content in disk
    // form. Names owned by a DISABLED mod get the same treatment even though
    // the registry knows them: their content is kept, not live, this
    // session, and nothing at runtime can encode to their disk ids. The
    // to-disk side is total after the append above, except for disabled
    // species (mob_to_disk = None: no live mob of theirs is ever saved).
    // Each direction must cover the wider of "every runtime id" and "every
    // disk id", so neither side can index past its table.
    let filled = |n: usize, v: u16| -> Box<[u16]> { vec![v; n].into_boxed_slice() };
    let block_len = Block::all().len().max(file.blocks.len());
    let item_len = ItemType::all().len().max(file.items.len());
    let mut p = Palette {
        block_to_disk: filled(block_len, 0),
        block_from_disk: filled(block_len, UNKNOWN),
        item_to_disk: filled(item_len, 0),
        item_from_disk: filled(item_len, UNKNOWN),
        mob_to_disk: [None; 256],
        mob_from_disk: [None; 256],
    };
    for (disk, name) in file.blocks.iter().enumerate() {
        match block_from_name(name) {
            Some(_) if name_disabled(name, disabled) => log::info!(
                "save palette: block '{name}' (disk id {disk}) is kept but not placed — its \
                 mod is disabled for this world"
            ),
            Some(b) => {
                p.block_from_disk[disk] = b.id();
                p.block_to_disk[b.id() as usize] = disk as u16;
            }
            None => log::warn!(
                "save palette: unknown block '{name}' (disk id {disk}) is kept but not \
                 placed — was this world last played on a newer or modded build?"
            ),
        }
    }
    for (disk, name) in file.items.iter().enumerate() {
        match item_from_name(name) {
            Some(_) if name_disabled(name, disabled) => log::info!(
                "save palette: item '{name}' (disk id {disk}) is kept but not usable — its \
                 mod is disabled for this world"
            ),
            Some(i) => {
                p.item_from_disk[disk] = i.id();
                p.item_to_disk[i.id() as usize] = disk as u16;
            }
            None => {
                log::warn!(
                    "save palette: unknown item '{name}' (disk id {disk}) is kept but not usable"
                )
            }
        }
    }
    for (disk, name) in file.mobs.iter().enumerate() {
        match mob_from_name(name) {
            Some(_) if name_disabled(name, disabled) => log::info!(
                "save palette: mob '{name}' (disk id {disk}) is kept but not spawned — its mod \
                 is disabled for this world"
            ),
            Some(m) => {
                p.mob_from_disk[disk] = Some(m.id());
                p.mob_to_disk[m.id() as usize] = Some(disk as u8);
            }
            None => log::warn!(
                "save palette: unknown mob '{name}' (disk id {disk}); such mobs are kept \
                 but not spawned (never respawned as a wrong species)"
            ),
        }
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> petramond_util::test_dirs::TestScratchDir {
        petramond_util::test_dirs::TestScratchDir::new(&format!("palette-{tag}"))
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
    }

    #[test]
    fn a_corrupt_palette_is_an_error_and_is_never_rewritten() {
        let dir = temp_dir("corrupt");
        std::fs::write(dir.join("palette.json"), b"{ not json").unwrap();
        let err = load_or_create(&dir, &no_disabled()).err().expect("refused");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(
            std::fs::read(dir.join("palette.json")).unwrap(),
            b"{ not json",
            "the file is left as found"
        );
    }

    #[test]
    fn a_palette_past_the_mob_ceiling_is_an_error_naming_the_mods() {
        let dir = temp_dir("mobceiling");
        let blocks: Vec<String> = Block::all().iter().map(|&b| block_name(b)).collect();
        let items: Vec<String> = ItemType::all().iter().map(|&i| item_name(i)).collect();
        let mut mobs: Vec<String> = (0..250).map(|i| format!("herdmod:m{i}")).collect();
        mobs.extend((0..20).map(|i| format!("swarmmod:m{i}")));
        let text = serde_json::to_string(&PaletteFile {
            blocks,
            items,
            mobs,
        })
        .unwrap();
        std::fs::write(dir.join("palette.json"), &text).unwrap();
        let err = load_or_create(&dir, &no_disabled()).err().expect("refused");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        let message = err.to_string();
        assert!(message.contains("swarmmod"), "{message}");
        assert!(!message.contains("herdmod"), "{message}");
        assert_eq!(
            std::fs::read_to_string(dir.join("palette.json")).unwrap(),
            text
        );
    }

    #[test]
    fn shuffled_palette_round_trips_and_remaps() {
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
    }

    #[test]
    fn unknown_disk_names_decode_to_air_and_registry_gets_appended() {
        let dir = temp_dir("unknown");
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
        assert_eq!(
            p.block_from_disk_known(1),
            None,
            "and is reported unknown, so the codec keeps it"
        );
        assert_eq!(p.block_from_disk_known(0), Some(0), "air is known");
        assert_eq!(p.block_from_disk_known(4000), None, "never pinned");
        for &b in Block::all() {
            assert_eq!(p.block_from_disk(p.block_to_disk(b.id())), b.id());
        }
    }

    #[test]
    fn pre_mob_palette_backfills_identity_and_pins_the_list() {
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
    }

    #[test]
    fn unknown_mob_names_decode_to_a_skip_and_known_ones_remap() {
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
    }

    #[test]
    fn disabled_mod_content_gets_the_unknown_treatment_and_reenabling_restores() {
        let root = petramond_util::test_dirs::TestScratchDir::new("paldis");
        let pack = root.join("mods/palmod");
        std::fs::create_dir_all(&pack).unwrap();
        std::fs::write(
            pack.join("pack.json"),
            r#"{ "name": "Palette Mod", "id": "palmod" }"#,
        )
        .unwrap();
        std::fs::write(
            pack.join("blocks.json"),
            r#"{ "blocks": [ { "block": "palmod:relic", "shape": "cube", "flags": ["solid", "opaque", "ao_occluder"], "tags": [], "behavior": "inert", "interaction": "none", "collision": [{"min": [0, 0, 0], "max": [1, 1, 1]}], "emission": 0, "tiles": ["stone", "stone", "stone"], "material": "stone", "data": {"petramond:harvest": {"tier": 1}}, "hardness": 2, "drops": [{"item": "palmod:relic", "min": 1, "max": 1, "chance": 1.0}] } ] }"#,
        )
        .unwrap();
        std::fs::write(
            pack.join("items.json"),
            r#"{ "items": [ { "item": "palmod:relic", "key": "palmod:relic", "name": "Relic", "max_stack_size": 64, "held_pose": {"pitch": 0, "yaw": 1.8, "roll": 0}, "tags": [], "block": "palmod:relic" } ] }"#,
        )
        .unwrap();

        crate::modding::tests::with_fixture_content(&root, || {
            disabled_mod_palette_inner(&root.join("save"))
        });
    }

    fn disabled_mod_palette_inner(save: &Path) {
        std::fs::create_dir_all(save).unwrap();
        let disabled: BTreeSet<String> = ["palmod".to_owned()].into();

        let relic = block_from_name("palmod:relic").expect("fixture block registered");
        let relic_item = item_from_name("palmod:relic").expect("fixture item registered");

        let p = load_or_create(save, &disabled).unwrap();
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

        let p = load_or_create(save, &BTreeSet::new()).unwrap();
        let disk = p.block_to_disk(relic.id());
        assert_ne!(disk, 0, "enabled content gets a real disk id");
        assert_eq!(p.block_from_disk(disk), relic.id());
        let item_disk = p.item_to_disk(relic_item.id());
        assert_eq!(p.item_from_disk(item_disk), relic_item.id());

        let p = load_or_create(save, &disabled).unwrap();
        assert_eq!(p.block_from_disk(disk), 0, "disabled block decodes to air");
        assert_eq!(p.block_from_disk_known(disk), None, "kept in disk form");
        assert_eq!(p.item_from_disk_known(item_disk), None);
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

        let p = load_or_create(save, &BTreeSet::new()).unwrap();
        assert_eq!(p.block_from_disk(disk), relic.id(), "re-enabling restores");
        assert_eq!(p.block_to_disk(relic.id()), disk, "same disk id as before");
        assert_eq!(p.item_from_disk(item_disk), relic_item.id());
    }
}
