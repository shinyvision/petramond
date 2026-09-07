//! Texture transitions: at the boundary between two neighbouring cube
//! materials, a displacement mask lets each face texel show either material,
//! so a dirt/grass seam reads as an organic edge instead of a straight line.
//!
//! Authored in `texture_transitions.json`, a layered POLICY catalog with two
//! row arrays:
//!
//! - `sets`: one row per edge style — the displacement `mask` (sixteen
//!   consecutive static atlas cells), its `width_texels`, and, derived from
//!   its members, one biome tint class. A pack adds an edge style by adding a
//!   set row.
//! - `pairs`: one row per unordered block pair that transitions, naming the
//!   set it belongs to. A pack adds pairs for its own blocks (against engine
//!   blocks or its own) and retires a pair it does not own with a patch row:
//!   `{"patch": "<pair>", "data": {"petramond:enabled": false}}`.
//!
//! Rows merge by key across layers (a later layer's row replaces an earlier
//! one) and expand `extends` templates like every other catalog. Nothing here
//! has an id space — no save, wire, or ABI value names a set or a pair — so,
//! like recipes, the catalog registers nothing and runs without a name table.
//!
//! A block may be a material of several sets. Every material of a set has a
//! 4-bit LOCAL id there, and a face is rendered in exactly one set (the mesher
//! picks the first set, in name order, that gives the face a donor).

use crate::{
    block::{Block, ShapeFamily},
    tile::{Tile, TileTint},
};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::LazyLock,
};

/// Sets a world can hold: the terrain vertex spends four bits on the set id.
pub const MAX_SETS: usize = 16;
/// Materials per set: local ids are four bits, and zero means "no donor".
pub const MAX_MATERIALS_PER_SET: usize = 15;
/// The displacement mask is a square grid of ordinary atlas cells.
pub const MASK_SIDE: usize = 4;
/// Half a tile: a wider band would let a texel border two opposite edges.
pub const MAX_WIDTH_TEXELS: u8 = 8;

/// A material's original albedo composition on one cube-face class.
#[derive(Clone, Copy, Debug)]
pub struct FaceMaterial {
    pub base: Tile,
    pub overlay: Option<Tile>,
}

/// One block's appearance inside a set. Local id = index + 1, in block-name
/// order, so ids never depend on registry or pack load order.
#[derive(Debug)]
pub struct Material {
    pub block: Block,
    /// Top, bottom, side, matching a block row's tile vocabulary.
    pub faces: [FaceMaterial; 3],
}

/// One edge style: a mask, a width, the materials that use it, and which of
/// them may bleed into which.
#[derive(Debug)]
pub struct Set {
    pub name: String,
    pub mask: Tile,
    pub width_texels: u8,
    /// Every tinted tile of every member shares this class (load-checked).
    pub tint: Option<TileTint>,
    pub materials: Vec<Material>,
    /// Bit `b` of entry `a`: local materials `a` and `b` transition.
    pairs: [u16; MAX_MATERIALS_PER_SET + 1],
}

impl Set {
    /// Pairs are symmetric; membership alone never implies a relationship.
    pub fn allows(&self, a: u8, b: u8) -> bool {
        a != 0
            && b != 0
            && (b as usize) <= MAX_MATERIALS_PER_SET
            && self.pairs[a as usize] & (1 << b) != 0
    }
}

/// A block's place in one set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Membership {
    pub set: u8,
    pub local: u8,
}

#[derive(Clone, Copy, Debug, Default)]
struct BlockEntry {
    first: u16,
    count: u8,
    tinted: bool,
}

/// Immutable policy, shared by mesh workers and shader construction.
#[derive(Debug)]
pub struct Rules {
    /// Name-ordered; a set's index is its id in the vertex payload.
    pub sets: Vec<Set>,
    memberships: Vec<Membership>,
    by_block: Box<[BlockEntry]>,
}

impl Rules {
    /// Every set `block` is a material of, in set order. Empty for a block
    /// outside the pair list — the per-cell test hot loops rely on.
    #[inline]
    pub fn memberships(&self, block: u16) -> &[Membership] {
        match self.by_block.get(block as usize) {
            Some(e) => &self.memberships[e.first as usize..e.first as usize + e.count as usize],
            None => &[],
        }
    }

    #[inline]
    pub fn is_material(&self, block: u16) -> bool {
        self.by_block
            .get(block as usize)
            .is_some_and(|e| e.count != 0)
    }

    /// Whether any set `block` belongs to carries a biome tint class.
    #[inline]
    pub fn is_tinted_material(&self, block: u16) -> bool {
        self.by_block.get(block as usize).is_some_and(|e| e.tinted)
    }

    /// `block`'s local id inside `set`, or zero when it is not a member.
    #[inline]
    pub fn local(&self, set: u8, block: u16) -> u8 {
        self.memberships(block)
            .iter()
            .find(|m| m.set == set)
            .map_or(0, |m| m.local)
    }

    /// Compile a catalog from its JSON layers (base first, packs after).
    pub fn from_layers(texts: &[&str]) -> Result<Rules, String> {
        compile(parse_layers(texts)?)
    }
}

/// Load once, after the ordinary block/tile catalogs are available.
pub fn rules() -> &'static Rules {
    static RULES: LazyLock<Rules> = LazyLock::new(|| {
        crate::registry::read_catalog(FILE, "texture transition", Rules::from_layers)
    });
    &RULES
}

const FILE: &str = "texture_transitions.json";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSet {
    set: String,
    mask: String,
    width_texels: u8,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPair {
    pair: String,
    set: String,
    blocks: [String; 2],
    #[serde(default)]
    data: serde_json::Map<String, serde_json::Value>,
}

struct Merged {
    sets: Vec<RawSet>,
    pairs: Vec<RawPair>,
}

/// Parse every layer once and merge both row arrays by key: a later layer's
/// row replaces an earlier one, patch rows collect for the enabled gate, and
/// retired pairs leave the catalog before compilation.
fn parse_layers(texts: &[&str]) -> Result<Merged, String> {
    fn merge<R>(rows: &mut Vec<R>, row: R, key: impl Fn(&R) -> &str) {
        match rows.iter().position(|r| key(r) == key(&row)) {
            Some(i) => rows[i] = row,
            None => rows.push(row),
        }
    }
    let mut sets: Vec<RawSet> = Vec::new();
    let mut pairs: Vec<RawPair> = Vec::new();
    let mut patches: Vec<crate::registry::RawDataPatch> = Vec::new();
    for (li, text) in texts.iter().enumerate() {
        let layer = || format!("layer #{li}");
        let file: serde_json::Value =
            serde_json::from_str(text).map_err(|e| format!("{}: invalid JSON: {e}", layer()))?;
        // A layer states only the arrays it contributes to.
        if file.get("sets").is_some() {
            for set in crate::registry::parse_rows_of::<RawSet>(&file, "sets", "set")
                .map_err(|e| format!("{}: {e}", layer()))?
            {
                merge(&mut sets, set, |r| &r.set);
            }
        }
        let rows = if file.get("pairs").is_some() {
            crate::registry::parse_rows_of::<serde_json::Value>(&file, "pairs", "pair")
                .map_err(|e| format!("{}: {e}", layer()))?
        } else {
            Vec::new()
        };
        for row in rows {
            if row.get("patch").is_some() {
                patches.push(serde_json::from_value(row).map_err(|e| format!("{}: {e}", layer()))?);
            } else {
                let pair: RawPair =
                    serde_json::from_value(row).map_err(|e| format!("{}: {e}", layer()))?;
                merge(&mut pairs, pair, |r| &r.pair);
            }
        }
    }
    for patch in &patches {
        if !pairs.iter().any(|p| p.pair == patch.patch) {
            return Err(format!("pair patch targets unknown pair '{}'", patch.patch));
        }
    }
    let mut enabled = Vec::with_capacity(pairs.len());
    for pair in pairs {
        let data = crate::registry::compile_data_map(&pair.pair, &pair.data, &patches)
            .map_err(|e| format!("pair '{}': {e}", pair.pair))?;
        if crate::registry::row_enabled(data).map_err(|e| format!("pair '{}': {e}", pair.pair))? {
            enabled.push(pair);
        } else {
            log::info!(
                "texture transition pair '{}' is retired by row data",
                pair.pair
            );
        }
    }
    Ok(Merged {
        sets,
        pairs: enabled,
    })
}

fn block_named(name: &str) -> Result<Block, String> {
    crate::registry::names()
        .blocks
        .id(name)
        .map(Block)
        .ok_or_else(|| format!("unknown transition block '{name}'"))
}

/// A material's face composition, or why the block cannot be one.
fn faces_of(block: Block, name: &str) -> Result<[FaceMaterial; 3], String> {
    if !block.is_opaque()
        || block.shape_family() != ShapeFamily::Cube
        || block.is_log()
        || block.front_tile().is_some()
    {
        return Err(format!(
            "transition block '{name}' must have ordinary opaque cube faces"
        ));
    }
    let mut faces = block.tiles().map(|base| FaceMaterial {
        base,
        overlay: None,
    });
    if let Some(side) = block.side_overlay() {
        faces[2] = FaceMaterial {
            base: side.base,
            overlay: Some(side.overlay),
        };
    }
    for face in &faces {
        for tile in std::iter::once(face.base).chain(face.overlay) {
            if tile.anim_frames() != 0 {
                return Err(format!(
                    "animated transition material '{name}' is unsupported"
                ));
            }
        }
    }
    Ok(faces)
}

fn compile(raw: Merged) -> Result<Rules, String> {
    if raw.sets.len() > MAX_SETS {
        return Err(format!(
            "{} transition sets declared; the terrain vertex addresses at most {MAX_SETS}",
            raw.sets.len()
        ));
    }
    // Name order everywhere: set ids and local material ids must not depend
    // on registry ids, row order, or which packs are enabled.
    let mut set_rows: Vec<RawSet> = raw.sets;
    set_rows.sort_by(|a, b| a.set.cmp(&b.set));
    let set_index = |name: &str| set_rows.iter().position(|s| s.set == name);

    let mut members: Vec<BTreeSet<String>> = vec![BTreeSet::new(); set_rows.len()];
    for pair in &raw.pairs {
        let Some(set) = set_index(&pair.set) else {
            return Err(format!(
                "pair '{}' names unknown transition set '{}'",
                pair.pair, pair.set
            ));
        };
        for name in &pair.blocks {
            if members[set].insert(name.clone()) && members[set].len() > MAX_MATERIALS_PER_SET {
                return Err(format!(
                    "transition set '{}' exceeds {MAX_MATERIALS_PER_SET} materials at '{name}'",
                    pair.set
                ));
            }
        }
    }

    let mut sets = Vec::with_capacity(set_rows.len());
    for (set, row) in set_rows.iter().enumerate() {
        if !(1..=MAX_WIDTH_TEXELS).contains(&row.width_texels) {
            return Err(format!(
                "transition set '{}': width must be 1..={MAX_WIDTH_TEXELS} texels",
                row.set
            ));
        }
        let mask = Tile::from_name(&row.mask)
            .ok_or_else(|| format!("transition set '{}': unknown mask '{}'", row.set, row.mask))?;
        if mask.anim_frames() != 0
            || mask.variation_count() != MASK_SIDE * MASK_SIDE
            || mask.world_tint().is_some()
        {
            return Err(format!(
                "transition set '{}': mask '{}' must be {} static, untinted atlas cells",
                row.set,
                row.mask,
                MASK_SIDE * MASK_SIDE
            ));
        }
        let mut tint = None;
        let mut materials = Vec::with_capacity(members[set].len());
        for name in &members[set] {
            let block = block_named(name)?;
            let faces = faces_of(block, name)?;
            for tile in faces
                .iter()
                .flat_map(|f| std::iter::once(f.base).chain(f.overlay))
            {
                if let Some(kind) = tile.world_tint() {
                    if tint.is_some_and(|current| current != kind) {
                        return Err(format!(
                            "transition set '{}': '{name}' mixes a second biome tint class into the set",
                            row.set
                        ));
                    }
                    tint = Some(kind);
                }
            }
            materials.push(Material { block, faces });
        }
        sets.push(Set {
            name: row.set.clone(),
            mask,
            width_texels: row.width_texels,
            tint,
            materials,
            pairs: [0; MAX_MATERIALS_PER_SET + 1],
        });
    }

    for pair in &raw.pairs {
        let set = &mut sets[set_index(&pair.set).expect("set resolved above")];
        let local = |name: &str| {
            let block = block_named(name).expect("pair blocks resolved above");
            set.materials
                .iter()
                .position(|m| m.block == block)
                .map(|i| i + 1)
                .expect("every pair block is a member of its set")
        };
        let (a, b) = (local(&pair.blocks[0]), local(&pair.blocks[1]));
        if a == b {
            return Err(format!("pair '{}' names one block twice", pair.pair));
        }
        if set.pairs[a] & (1 << b) != 0 {
            return Err(format!(
                "pair '{}' restates a pair already declared in set '{}'",
                pair.pair, pair.set
            ));
        }
        set.pairs[a] |= 1 << b;
        set.pairs[b] |= 1 << a;
    }

    let mut per_block: BTreeMap<u16, Vec<Membership>> = BTreeMap::new();
    for (set, s) in sets.iter().enumerate() {
        for (i, m) in s.materials.iter().enumerate() {
            per_block.entry(m.block.id()).or_default().push(Membership {
                set: set as u8,
                local: (i + 1) as u8,
            });
        }
    }
    let mut memberships = Vec::new();
    let mut by_block = vec![BlockEntry::default(); Block::all().len()].into_boxed_slice();
    for (block, list) in per_block {
        by_block[block as usize] = BlockEntry {
            first: memberships.len() as u16,
            count: list.len() as u8,
            tinted: list.iter().any(|m| sets[m.set as usize].tint.is_some()),
        };
        memberships.extend(list);
    }
    Ok(Rules {
        sets,
        memberships,
        by_block,
    })
}

#[cfg(test)]
mod tests;
