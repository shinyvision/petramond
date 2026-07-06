//! Load block definitions from `assets/blocks.json` (serde).
//!
//! Every block's data row lives on disk (like `recipes.json`), so block
//! properties are editable — and moddable — without a rebuild. Rows reference
//! blocks/items by their registry names, tags/materials by their snake_case
//! serde names, tiles by their atlas asset names, and behaviours by their
//! [`BlockBehavior::key`] names.
//!
//! Two kinds of row (see [`crate::registry`]): a row whose key is an ENGINE
//! block name overrides that block's def (a pack states only the rows it
//! changes); a row with a NAMESPACED key (`mod_id:name`) REGISTERS a new
//! dynamic block at the next free id. A new bare name is an error.
//!
//! Unlike recipes (where a malformed row is skipped), the block table is
//! load-bearing for the whole engine — world gen, meshing, lighting, and save
//! decode all index it by block id — so the loader validates that the file
//! covers EVERY registered block exactly once and fails loudly at startup on
//! any mismatch, rather than limping on with a partial table.
//!
//! [`BlockBehavior::key`]: super::behavior::BlockBehavior::key

use serde::{Deserialize, Serialize};

use crate::atlas::Tile;
use crate::item::{Drop, DropSpec, ItemType};
use crate::registry::ContentNames;

use super::definition::{BlockDef, BlockFlags, BlockMaterial, BlockParticleEmitter};
use super::{behavior, Aabb, Block, BlockInteraction, BlockTag, RenderShape};

#[derive(Serialize, Deserialize)]
pub(super) struct RawFile {
    pub blocks: Vec<RawBlockDef>,
}

/// One block row as written in `blocks.json`: a field-for-field mirror of
/// [`BlockDef`] with names in place of ids/pointers (the block itself, drops'
/// items, tiles, behaviour) and owned `Vec`s in place of `'static` slices.
/// Floats ride as `f64` (JSON's native width); converting narrows back to the
/// exact `f32` the shortest decimal representation denotes.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawBlockDef {
    /// Registry name: an engine block name (override) or a namespaced
    /// `mod_id:name` key (dynamic registration). Resolved against the name
    /// table, NOT through `Block` serde, so the loader stays the one place
    /// ids are assigned.
    pub block: String,
    pub shape: RenderShape,
    pub flags: Vec<RawFlag>,
    /// Tag names: bare engine tags or namespaced `mod_id:name` pack tags
    /// (interned at load — see [`BlockTag::resolve`]).
    pub tags: Vec<String>,
    pub behavior: String,
    pub interaction: RawInteraction,
    pub collision: Vec<Aabb>,
    pub emission: u8,
    #[serde(default)]
    pub particle_emitter: Option<BlockParticleEmitter>,
    pub tiles: [String; 3],
    pub material: BlockMaterial,
    pub harvest_tier: u8,
    pub hardness: f64,
    pub drops: Vec<RawDrop>,
}

/// A row's `interaction` field: a bare engine action name (`"none"`,
/// `"open_furnace"`, ...) or `{"open_gui": "mod_id:name"}` opening a
/// mod-defined GUI kind. Resolved to [`BlockInteraction`] in [`convert`].
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum RawInteraction {
    Named(String),
    OpenGui { open_gui: String },
}

impl RawInteraction {
    fn resolve(&self) -> Result<BlockInteraction, String> {
        match self {
            RawInteraction::Named(name) => Ok(match name.as_str() {
                "none" => BlockInteraction::None,
                "open_crafting_table" => BlockInteraction::OpenCraftingTable,
                "open_furnace" => BlockInteraction::OpenFurnace,
                "open_chest" => BlockInteraction::OpenChest,
                "open_furniture_workbench" => BlockInteraction::OpenFurnitureWorkbench,
                "toggle_door" => BlockInteraction::ToggleDoor,
                "sleep" => BlockInteraction::Sleep,
                other => return Err(format!("unknown interaction '{other}'")),
            }),
            RawInteraction::OpenGui { open_gui } => {
                // Mod GUI kinds must be namespaced (engine screens carry
                // session/slot semantics an open_gui row cannot provide).
                if !crate::registry::is_namespaced(open_gui) {
                    return Err(format!(
                        "open_gui '{open_gui}' must be a namespaced 'mod_id:name' GUI kind"
                    ));
                }
                let kind = crate::gui::intern_kind(open_gui)
                    .ok_or_else(|| format!("cannot register gui kind '{open_gui}'"))?;
                Ok(BlockInteraction::OpenModGui(kind))
            }
        }
    }
}

/// A [`BlockFlags`] bit by name — rows list the flags they carry.
#[derive(Copy, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RawFlag {
    Solid,
    Opaque,
    AoOccluder,
    Transparent,
    DirectionalView,
}

impl RawFlag {
    fn to_flag(self) -> BlockFlags {
        match self {
            RawFlag::Solid => BlockFlags::SOLID,
            RawFlag::Opaque => BlockFlags::OPAQUE,
            RawFlag::AoOccluder => BlockFlags::AO_OCCLUDER,
            RawFlag::Transparent => BlockFlags::TRANSPARENT,
            RawFlag::DirectionalView => BlockFlags::DIRECTIONAL_VIEW,
        }
    }
}

/// One entry of a row's `drops` list (mirror of [`Drop`]; `item` is the
/// dropped item's registry name).
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawDrop {
    pub item: String,
    pub min: u8,
    pub max: u8,
    pub chance: f64,
}

/// The loaded block table: id-indexed defs plus the dense per-id flag copy the
/// mesher/light hot loops read (see `data::flags`).
pub(super) struct Registry {
    pub defs: &'static [BlockDef],
    pub flags: [BlockFlags; 256],
}

/// Load the registry from every `blocks.json` layer (base + mod packs, later
/// packs replacing rows by block — see [`crate::assets::read_layers`]),
/// panicking with a precise message if the table is missing or inconsistent.
pub(super) fn registry() -> Registry {
    let layers = crate::assets::read_layers("blocks.json");
    if layers.is_empty() {
        panic!(
            "blocks.json not found (searched {:?}); the game cannot run without its block table",
            crate::assets::candidate_paths("blocks.json")
        );
    }
    for (_, path) in &layers {
        log::info!("block defs layer: {}", path.display());
    }
    let texts: Vec<&str> = layers.iter().map(|(s, _)| s.as_str()).collect();
    // The global name table was built from these same layers, so every row key
    // resolves and every dynamic id is already assigned.
    parse_layers(&texts, crate::registry::names()).unwrap_or_else(|e| panic!("blocks.json: {e}"))
}

#[cfg(test)]
pub(super) fn parse(text: &str) -> Result<Registry, String> {
    parse_test_layers(&[text])
}

/// Test harness: parse synthetic layers against a name table built from those
/// same layers (+ the shipped items for drop resolution), mirroring the real
/// bootstrap without touching the global registries.
#[cfg(test)]
pub(super) fn parse_test_layers(texts: &[&str]) -> Result<Registry, String> {
    let (items, _) =
        crate::assets::read_base_text("items.json").expect("assets/items.json must ship");
    let names = crate::registry::build_names(texts, &[&items])?;
    parse_layers(texts, &names)
}

pub(super) fn parse_layers(texts: &[&str], names: &ContentNames) -> Result<Registry, String> {
    // Merge layers by block key: a later layer's row REPLACES the earlier one,
    // so a mod pack states only the rows it changes (or adds).
    let mut merged: Vec<RawBlockDef> = Vec::new();
    for (li, text) in texts.iter().enumerate() {
        let raw: RawFile =
            serde_json::from_str(text).map_err(|e| format!("layer #{li}: invalid JSON: {e}"))?;
        for r in raw.blocks {
            match merged.iter_mut().find(|m| m.block == r.block) {
                Some(slot) => *slot = r,
                None => merged.push(r),
            }
        }
    }
    let expected = names.blocks.len();
    let mut rows: Vec<Option<BlockDef>> = (0..expected).map(|_| None).collect();
    for r in merged {
        let id = names
            .blocks
            .id(&r.block)
            .ok_or_else(|| format!("unregistered block '{}'", r.block))?;
        let key = r.block.clone();
        rows[id as usize] =
            Some(convert(r, Block(id), names).map_err(|e| format!("block '{key}': {e}"))?);
    }
    // Ids are assigned contiguously by the name table, so covering every
    // registered name exactly once fills 0..expected with no holes.
    let mut defs = Vec::with_capacity(expected);
    for (id, row) in rows.into_iter().enumerate() {
        defs.push(row.ok_or_else(|| {
            format!(
                "missing row for block '{}'",
                names.blocks.name(id as u8).unwrap_or("?")
            )
        })?);
    }
    let defs: &'static [BlockDef] = Box::leak(defs.into_boxed_slice());
    let mut flags = [BlockFlags::NONE; 256];
    for d in defs {
        flags[d.block.id() as usize] = d.flags;
    }
    Ok(Registry { defs, flags })
}

fn convert(r: RawBlockDef, block: Block, names: &ContentNames) -> Result<BlockDef, String> {
    let behavior = behavior::by_name(&r.behavior)
        .ok_or_else(|| format!("unknown behavior '{}'", r.behavior))?;
    let tile = |name: &String| -> Result<Tile, String> {
        Tile::from_name(name).ok_or_else(|| format!("unknown tile '{name}'"))
    };
    let tiles = [tile(&r.tiles[0])?, tile(&r.tiles[1])?, tile(&r.tiles[2])?];
    let mut flags = BlockFlags::NONE;
    for f in &r.flags {
        flags = flags.with(f.to_flag());
    }
    let drops: Vec<Drop> = r
        .drops
        .iter()
        .map(|d| {
            let item = names
                .items
                .id(&d.item)
                .map(ItemType)
                .ok_or_else(|| format!("unknown drop item '{}'", d.item))?;
            Ok(Drop {
                item,
                min: d.min,
                max: d.max,
                chance: d.chance as f32,
            })
        })
        .collect::<Result<_, String>>()?;
    let tags: Vec<BlockTag> = r
        .tags
        .iter()
        .map(|t| BlockTag::resolve(t))
        .collect::<Result<_, String>>()?;
    if let Some(emitter) = &r.particle_emitter {
        validate_particle_emitter(emitter)?;
    }
    Ok(BlockDef {
        block,
        flags,
        tags: leak(tags),
        behavior,
        interaction: r.interaction.resolve()?,
        shape: r.shape,
        collision: leak(r.collision),
        emission: r.emission,
        particle_emitter: r.particle_emitter,
        tiles,
        material: r.material,
        harvest_tier: r.harvest_tier,
        hardness: r.hardness as f32,
        drop: DropSpec { drops: leak(drops) },
    })
}

fn validate_particle_emitter(e: &BlockParticleEmitter) -> Result<(), String> {
    let finite = |label: &str, value: f32| -> Result<(), String> {
        if value.is_finite() {
            Ok(())
        } else {
            Err(format!("particle_emitter.{label} must be finite"))
        }
    };
    let ordered_positive = |label: &str, range: [f32; 2]| -> Result<(), String> {
        finite(&format!("{label}[0]"), range[0])?;
        finite(&format!("{label}[1]"), range[1])?;
        if range[0] <= 0.0 || range[1] <= 0.0 {
            return Err(format!("particle_emitter.{label} values must be > 0"));
        }
        if range[0] > range[1] {
            return Err(format!("particle_emitter.{label} min must be <= max"));
        }
        Ok(())
    };
    ordered_positive("rate", e.rate)?;
    ordered_positive("lifetime", e.lifetime)?;
    ordered_positive("size", e.size)?;

    for (label, values) in [
        ("origin", e.origin.as_slice()),
        ("offset", e.offset.as_slice()),
        ("spawn_box", e.spawn_box.as_slice()),
        ("velocity", e.velocity.as_slice()),
        ("velocity_jitter", e.velocity_jitter.as_slice()),
    ] {
        for (i, &value) in values.iter().enumerate() {
            finite(&format!("{label}[{i}]"), value)?;
        }
    }
    for (label, values) in [
        ("spawn_box", e.spawn_box),
        ("velocity_jitter", e.velocity_jitter),
    ] {
        for (i, value) in values.into_iter().enumerate() {
            if value < 0.0 {
                return Err(format!("particle_emitter.{label}[{i}] must be >= 0"));
            }
        }
    }
    for (endpoint, color) in e.color.into_iter().enumerate() {
        for (channel, value) in color.into_iter().enumerate() {
            finite(&format!("color[{endpoint}][{channel}]"), value)?;
            if !(0.0..=1.0).contains(&value) {
                return Err("particle_emitter.color channels must be in 0..=1".into());
            }
        }
    }
    for (i, value) in e.alpha.into_iter().enumerate() {
        finite(&format!("alpha[{i}]"), value)?;
        if !(0.0..=1.0).contains(&value) {
            return Err("particle_emitter.alpha values must be in 0..=1".into());
        }
    }
    if e.alpha[0] > e.alpha[1] {
        return Err("particle_emitter.alpha min must be <= max".into());
    }
    Ok(())
}

/// The table loads exactly once per process (a `LazyLock` in `data`), so its
/// rows' slices may leak into `'static` — keeping every [`BlockDef`] consumer
/// signature identical to the old compiled-in table.
fn leak<T>(v: Vec<T>) -> &'static [T] {
    Box::leak(v.into_boxed_slice())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped `assets/blocks.json` must load fully — the same gate the game
    /// applies at startup, surfaced as a test so a bad edit fails CI, not a launch.
    #[test]
    fn shipped_blocks_json_loads_fully() {
        let (text, path) =
            crate::assets::read_base_text("blocks.json").expect("assets/blocks.json must ship");
        let reg = parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(
            reg.defs.len(),
            crate::block::ENGINE_BLOCK_NAMES.len(),
            "the base table is exactly the engine set"
        );
    }

    #[test]
    fn pack_layer_overrides_rows_by_block() {
        let (base, _) =
            crate::assets::read_base_text("blocks.json").expect("assets/blocks.json must ship");
        let layer = r#"{ "blocks": [ { "block": "llama:stone", "shape": "cube", "flags": ["solid", "opaque", "ao_occluder"], "tags": ["terrain"], "behavior": "inert", "interaction": "none", "collision": [{"min": [0, 0, 0], "max": [1, 1, 1]}], "emission": 0, "tiles": ["stone", "stone", "stone"], "material": "stone", "harvest_tier": 1, "hardness": 99, "drops": [] } ] }"#;
        let base_reg = parse(&base).expect("base table loads");
        let reg = parse_test_layers(&[&base, layer]).expect("layered table loads");
        assert_eq!(
            reg.defs[Block::Stone.id() as usize].hardness,
            99.0,
            "the pack layer's stone row replaces the base row"
        );
        // An override registers no new id.
        assert_eq!(reg.defs.len(), crate::block::ENGINE_BLOCK_NAMES.len());
        // Rows the layer does not name are untouched.
        assert_eq!(
            reg.defs[Block::Dirt.id() as usize].hardness,
            base_reg.defs[Block::Dirt.id() as usize].hardness
        );
    }

    #[test]
    fn namespaced_pack_row_registers_a_new_block() {
        let (base, _) =
            crate::assets::read_base_text("blocks.json").expect("assets/blocks.json must ship");
        let layer = r#"{ "blocks": [ { "block": "mymod:glowrock", "shape": "cube", "flags": ["solid", "opaque", "ao_occluder"], "tags": [], "behavior": "inert", "interaction": "none", "collision": [{"min": [0, 0, 0], "max": [1, 1, 1]}], "emission": 28, "tiles": ["stone", "stone", "stone"], "material": "stone", "harvest_tier": 1, "hardness": 2, "drops": [{"item": "llama:cobblestone", "min": 1, "max": 1, "chance": 1.0}] } ] }"#;
        let reg = parse_test_layers(&[&base, layer]).expect("dynamic row loads");
        let engine = crate::block::ENGINE_BLOCK_NAMES.len();
        assert_eq!(
            reg.defs.len(),
            engine + 1,
            "one fresh id past the engine set"
        );
        let def = &reg.defs[engine];
        assert_eq!(def.block, Block(engine as u8));
        // The row's properties resolve like any engine row's.
        assert!(def.flags.is_solid() && def.flags.is_opaque());
        assert_eq!(def.behavior.key(), "inert");
        assert_eq!(def.emission, 28);
        assert_eq!(def.drop.drops.len(), 1);
        assert_eq!(def.drop.drops[0].item, ItemType::Cobblestone);
        // Engine ids are untouched by the addition.
        assert_eq!(reg.defs[Block::Stone.id() as usize].block, Block::Stone);
    }

    #[test]
    fn namespaced_block_rows_can_declare_particle_emitters() {
        let (base, _) =
            crate::assets::read_base_text("blocks.json").expect("assets/blocks.json must ship");
        let layer = r#"{ "blocks": [ { "block": "mymod:spark", "shape": "cube", "flags": ["solid"], "tags": [], "behavior": "inert", "interaction": "none", "collision": [{"min": [0, 0, 0], "max": [1, 1, 1]}], "emission": 0, "particle_emitter": { "anchor": "block_top", "rate": [1.0, 2.0], "lifetime": [0.2, 0.4], "size": [0.02, 0.05], "spawn_box": [0.1, 0.0, 0.1], "velocity": [0.0, 0.2, 0.0], "velocity_jitter": [0.03, 0.02, 0.03], "color": [[1.0, 0.2, 0.0], [1.0, 1.0, 0.2]], "alpha": [0.2, 0.6], "fullbright": true }, "tiles": ["stone", "stone", "stone"], "material": "stone", "harvest_tier": 1, "hardness": 2, "drops": [] } ] }"#;
        let reg = parse_test_layers(&[&base, layer]).expect("particle emitter row loads");
        let def = &reg.defs[crate::block::ENGINE_BLOCK_NAMES.len()];
        assert!(
            def.particle_emitter.is_some(),
            "dynamic block row carries its emitter into the loaded definition"
        );
    }

    #[test]
    fn particle_emitter_rows_validate_ranges() {
        let (base, _) =
            crate::assets::read_base_text("blocks.json").expect("assets/blocks.json must ship");
        let layer = r#"{ "blocks": [ { "block": "mymod:bad_spark", "shape": "cube", "flags": [], "tags": [], "behavior": "inert", "interaction": "none", "collision": [], "emission": 0, "particle_emitter": { "rate": 2.0, "lifetime": [0.5, 0.2], "size": [0.02, 0.05], "color": [[1.0, 0.2, 0.0], [1.0, 1.0, 0.2]], "alpha": [0.2, 0.6] }, "tiles": ["stone", "stone", "stone"], "material": "stone", "harvest_tier": 0, "hardness": 1, "drops": [] } ] }"#;
        let err = parse_test_layers(&[&base, layer])
            .err()
            .expect("reversed lifetime is rejected");
        assert!(err.contains("particle_emitter.lifetime"), "{err}");
    }

    #[test]
    fn new_bare_name_rows_are_rejected() {
        let (base, _) =
            crate::assets::read_base_text("blocks.json").expect("assets/blocks.json must ship");
        let layer = r#"{ "blocks": [ { "block": "glowrock", "shape": "cube", "flags": [], "tags": [], "behavior": "inert", "interaction": "none", "collision": [], "emission": 0, "tiles": ["stone", "stone", "stone"], "material": "none", "harvest_tier": 0, "hardness": 1, "drops": [] } ] }"#;
        let err = parse_test_layers(&[&base, layer])
            .err()
            .expect("bare additions are refused");
        assert!(
            err.contains("glowrock") && err.contains("namespace"),
            "{err}"
        );
    }

    /// Phase 5: `interaction: {"open_gui": "mod:kind"}` resolves to
    /// `OpenModGui` with a registered kind; a bare (un-namespaced) open_gui
    /// key and an unknown named interaction are load errors.
    #[test]
    fn open_gui_interaction_parses_namespaced_and_rejects_bare() {
        let (base, _) =
            crate::assets::read_base_text("blocks.json").expect("assets/blocks.json must ship");
        let layer = r#"{ "blocks": [ { "block": "guimod:opener", "shape": "cube", "flags": ["solid", "opaque", "ao_occluder"], "tags": [], "behavior": "inert", "interaction": {"open_gui": "guimod:panel"}, "collision": [{"min": [0, 0, 0], "max": [1, 1, 1]}], "emission": 0, "tiles": ["stone", "stone", "stone"], "material": "stone", "harvest_tier": 1, "hardness": 2, "drops": [] } ] }"#;
        let reg = parse_test_layers(&[&base, layer]).expect("open_gui row loads");
        let def = &reg.defs[crate::block::ENGINE_BLOCK_NAMES.len()];
        let BlockInteraction::OpenModGui(kind) = def.interaction else {
            panic!("expected OpenModGui, got {:?}", def.interaction);
        };
        assert_eq!(crate::gui::kind_key(kind), Some("guimod:panel"));

        let bare = layer.replace("guimod:panel", "panel");
        let err = parse_test_layers(&[&base, &bare]).err().unwrap();
        assert!(err.contains("namespaced"), "{err}");

        let unknown = layer.replace(r#"{"open_gui": "guimod:panel"}"#, r#""bogus_action""#);
        let err = parse_test_layers(&[&base, &unknown]).err().unwrap();
        assert!(err.contains("unknown interaction"), "{err}");
    }

    #[test]
    fn loader_rejects_incomplete_or_unknown_rows() {
        // A single valid row is not a full table: the error names a missing block.
        let partial = r#"{ "blocks": [ { "block": "llama:air", "shape": "cube", "flags": [], "tags": [], "behavior": "inert", "interaction": "none", "collision": [], "emission": 0, "tiles": ["dirt", "dirt", "dirt"], "material": "none", "harvest_tier": 0, "hardness": -1, "drops": [] } ] }"#;
        assert!(parse(partial).err().unwrap().contains("missing row"));
        // Unknown behavior name (the full base table with one row broken).
        let (base, _) =
            crate::assets::read_base_text("blocks.json").expect("assets/blocks.json must ship");
        let bad_behavior = r#"{ "blocks": [ { "block": "llama:air", "shape": "cube", "flags": [], "tags": [], "behavior": "bogus", "interaction": "none", "collision": [], "emission": 0, "tiles": ["dirt", "dirt", "dirt"], "material": "none", "harvest_tier": 0, "hardness": -1, "drops": [] } ] }"#;
        assert!(parse_test_layers(&[&base, bad_behavior])
            .err()
            .unwrap()
            .contains("unknown behavior"));
        // Unknown tile name.
        let bad_tile = bad_behavior.replace("\"bogus\"", "\"inert\"").replace(
            "\"dirt\", \"dirt\", \"dirt\"",
            "\"dirt\", \"dirt\", \"bogus_tile\"",
        );
        assert!(parse_test_layers(&[&base, &bad_tile])
            .err()
            .unwrap()
            .contains("unknown tile"));
        // Unknown drop item name.
        let bad_drop = bad_behavior.replace("\"bogus\"", "\"inert\"").replace(
            "\"drops\": []",
            "\"drops\": [{\"item\": \"bogus_item\", \"min\": 1, \"max\": 1, \"chance\": 1.0}]",
        );
        assert!(parse_test_layers(&[&base, &bad_drop])
            .err()
            .unwrap()
            .contains("unknown drop item"));
    }
}
