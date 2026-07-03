//! Load block definitions from `assets/blocks.json` (serde).
//!
//! Every block's data row lives on disk (like `recipes.json`), so block
//! properties are editable — and moddable — without a rebuild. Rows reference
//! blocks/tags/materials by their snake_case serde names, tiles by their atlas
//! asset names, and behaviours by their [`BlockBehavior::key`] names.
//!
//! Unlike recipes (where a malformed row is skipped), the block table is
//! load-bearing for the whole engine — world gen, meshing, lighting, and save
//! decode all index it by block id — so the loader validates that the file
//! covers EVERY `Block` exactly once and fails loudly at startup on any
//! mismatch, rather than limping on with a partial table.
//!
//! [`BlockBehavior::key`]: super::behavior::BlockBehavior::key

use serde::{Deserialize, Serialize};

use crate::atlas::Tile;
use crate::item::{Drop, DropSpec, ItemType};

use super::definition::{BlockDef, BlockFlags, BlockMaterial};
use super::{behavior, Aabb, Block, BlockInteraction, BlockTag, RenderShape};

#[derive(Serialize, Deserialize)]
pub(super) struct RawFile {
    pub blocks: Vec<RawBlockDef>,
}

/// One block row as written in `blocks.json`: a field-for-field mirror of
/// [`BlockDef`] with names in place of pointers (tiles, behaviour) and owned
/// `Vec`s in place of `'static` slices. Floats ride as `f64` (JSON's native
/// width); converting narrows back to the exact `f32` the shortest decimal
/// representation denotes.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawBlockDef {
    pub block: Block,
    pub shape: RenderShape,
    pub flags: Vec<RawFlag>,
    pub tags: Vec<BlockTag>,
    pub behavior: String,
    pub interaction: BlockInteraction,
    pub collision: Vec<Aabb>,
    pub emission: u8,
    pub tiles: [String; 3],
    pub material: BlockMaterial,
    pub harvest_tier: u8,
    pub hardness: f64,
    pub drops: Vec<RawDrop>,
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

/// One entry of a row's `drops` list (mirror of [`Drop`]).
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawDrop {
    pub item: ItemType,
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
    parse_layers(&texts).unwrap_or_else(|e| panic!("blocks.json: {e}"))
}

#[cfg(test)]
pub(super) fn parse(text: &str) -> Result<Registry, String> {
    parse_layers(&[text])
}

pub(super) fn parse_layers(texts: &[&str]) -> Result<Registry, String> {
    // Merge layers by block: a later layer's row REPLACES the earlier one, so a
    // mod pack states only the rows it changes.
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
    let expected = Block::ALL.len();
    let mut rows: Vec<Option<BlockDef>> = (0..expected).map(|_| None).collect();
    for r in merged {
        let block = r.block;
        let id = block.id() as usize;
        rows[id] = Some(convert(r).map_err(|e| format!("block {block:?}: {e}"))?);
    }
    // Block ids are the contiguous enum discriminants, so covering every
    // variant exactly once fills 0..expected with no holes.
    let mut defs = Vec::with_capacity(expected);
    for (id, row) in rows.into_iter().enumerate() {
        defs.push(row.ok_or_else(|| format!("missing row for block {:?}", Block::ALL[id]))?);
    }
    let defs: &'static [BlockDef] = Box::leak(defs.into_boxed_slice());
    let mut flags = [BlockFlags::NONE; 256];
    for d in defs {
        flags[d.block.id() as usize] = d.flags;
    }
    Ok(Registry { defs, flags })
}

fn convert(r: RawBlockDef) -> Result<BlockDef, String> {
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
        .map(|d| Drop {
            item: d.item,
            min: d.min,
            max: d.max,
            chance: d.chance as f32,
        })
        .collect();
    Ok(BlockDef {
        block: r.block,
        flags,
        tags: leak(r.tags),
        behavior,
        interaction: r.interaction,
        shape: r.shape,
        collision: leak(r.collision),
        emission: r.emission,
        tiles,
        material: r.material,
        harvest_tier: r.harvest_tier,
        hardness: r.hardness as f32,
        drop: DropSpec { drops: leak(drops) },
    })
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
            crate::assets::read_text("blocks.json").expect("assets/blocks.json must ship");
        parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    }

    #[test]
    fn pack_layer_overrides_rows_by_block() {
        let (base, _) =
            crate::assets::read_text("blocks.json").expect("assets/blocks.json must ship");
        let layer = r#"{ "blocks": [ { "block": "stone", "shape": "cube", "flags": ["solid", "opaque", "ao_occluder"], "tags": ["terrain"], "behavior": "inert", "interaction": "none", "collision": [{"min": [0, 0, 0], "max": [1, 1, 1]}], "emission": 0, "tiles": ["stone", "stone", "stone"], "material": "stone", "harvest_tier": 1, "hardness": 99, "drops": [] } ] }"#;
        let reg = parse_layers(&[&base, layer]).expect("layered table loads");
        assert_eq!(
            reg.defs[Block::Stone.id() as usize].hardness,
            99.0,
            "the pack layer's stone row replaces the base row"
        );
        // Rows the layer does not name are untouched.
        assert_eq!(reg.defs[Block::Dirt.id() as usize].hardness, 0.5);
    }

    #[test]
    fn loader_rejects_incomplete_or_unknown_rows() {
        // Unknown block name: serde's enum error names the bad variant.
        let bad = r#"{ "blocks": [ { "block": "bogus_block", "shape": "cube", "flags": [], "tags": [], "behavior": "inert", "interaction": "none", "collision": [], "emission": 0, "tiles": ["dirt", "dirt", "dirt"], "material": "none", "harvest_tier": 0, "hardness": 1, "drops": [] } ] }"#;
        assert!(parse(bad).err().unwrap().contains("bogus_block"));
        // A single valid row is not a full table: the error names a missing block.
        let partial = r#"{ "blocks": [ { "block": "air", "shape": "cube", "flags": [], "tags": [], "behavior": "inert", "interaction": "none", "collision": [], "emission": 0, "tiles": ["dirt", "dirt", "dirt"], "material": "none", "harvest_tier": 0, "hardness": -1, "drops": [] } ] }"#;
        assert!(parse(partial).err().unwrap().contains("missing row"));
        // Unknown behavior name.
        let bad_behavior = partial.replace("\"inert\"", "\"bogus\"");
        assert!(parse(&bad_behavior).err().unwrap().contains("unknown behavior"));
        // Unknown tile name.
        let bad_tile =
            partial.replace("\"dirt\", \"dirt\", \"dirt\"", "\"dirt\", \"dirt\", \"bogus_tile\"");
        assert!(parse(&bad_tile).err().unwrap().contains("unknown tile"));
    }
}
