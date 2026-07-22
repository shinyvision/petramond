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
use crate::facing::Facing;
use crate::item::{Drop, DropSpec, ItemType};
use crate::registry::ContentNames;

use super::definition::{self, BlockDef, BlockFlags, BlockMaterial, ParticleEmitter};
use super::shape_kind::{self, RawShape, ShapeFamily, ShapeKindDef, ShapeKindInterner};
use super::{behavior, Aabb, Block, BlockInteraction, BlockTag};

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
    pub shape: RawShape,
    pub flags: Vec<RawFlag>,
    /// Tag names: bare engine tags or namespaced `mod_id:name` pack tags
    /// (interned at load — see [`BlockTag::resolve`]).
    pub tags: Vec<String>,
    pub behavior: String,
    pub interaction: RawInteraction,
    pub collision: Vec<Aabb>,
    pub emission: u8,
    #[serde(default)]
    pub particle_emitter: Option<RawEmitterRef>,
    pub tiles: [String; 3],
    /// Tile shown on the placed entity-facing face (furnace/chest fronts).
    /// Only valid together with the `directional_view` flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub front: Option<String>,
    /// Side compositing: `{"base": tile, "overlay": tile}` — side faces draw
    /// the base with the overlay tinted by its atlas tint class (grass).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side_overlay: Option<RawSideOverlay>,
    /// Side tile swapped in while a `snow_cover` block sits directly above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub covered_side: Option<String>,
    pub material: BlockMaterial,
    pub harvest_tier: u8,
    pub hardness: f64,
    pub drops: Vec<RawDrop>,
    /// Sapling stage chain: the registry name of the block this row advances
    /// to on a successful growth roll. Required on every NON-final `sapling`
    /// behaviour row, forbidden anywhere else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_stage: Option<String>,
    /// The tree(s) a FINAL sapling stage grows: a `features.json` key, or a
    /// weighted list of them. Required on every final `sapling` behaviour row,
    /// forbidden anywhere else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grows_into: Option<RawGrowsInto>,
    /// A ladder-shaped row's fixed wall facing (`"north"` / `"south"` /
    /// `"west"` / `"east"`): the direction the panel front points, away from
    /// its supporting wall. Required on every `ladder`-shaped row, forbidden
    /// anywhere else — facing is block identity, one row per facing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel_facing: Option<String>,
    /// The facing → sibling-row map of a wall-panel family's placeable row
    /// (all four directions required); placement commits the sibling matching
    /// the clicked face's normal. Only valid on `ladder`-shaped rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facing_rows: Option<RawFacingRows>,
}

/// A row's `facing_rows` field: the four facing-sibling registry names.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawFacingRows {
    pub north: String,
    pub south: String,
    pub west: String,
    pub east: String,
}

/// A row's `side_overlay` field: the two tiles of a composited side face.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawSideOverlay {
    pub base: String,
    pub overlay: String,
}

/// A row's `grows_into` field: one feature key, or a weighted choice list
/// (`[{"feature": "petramond:oak_big", "weight": 1}, ...]`; `weight` defaults
/// to 1).
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum RawGrowsInto {
    Key(String),
    Weighted(Vec<RawGrowthChoice>),
}

/// One weighted `grows_into` entry.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawGrowthChoice {
    pub feature: String,
    #[serde(default = "default_growth_weight")]
    pub weight: f64,
}

fn default_growth_weight() -> f64 {
    1.0
}

/// A row's `particle_emitter` field: a `particle_emitters.json` bundle KEY
/// (`"petramond:torch_flame"` — the reusable, mob-shareable form), or one inline
/// anonymous row for a block-local one-off. A referenced bundle contributes all
/// its particle rows; its `tint` is mob-body data and blocks ignore it.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum RawEmitterRef {
    Key(String),
    Inline(ParticleEmitter),
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
                // The named engine openers are vocabulary sugar: they resolve
                // to the same unified shape mod `open_gui` rows use.
                "open_crafting_table" => {
                    BlockInteraction::OpenGui(crate::gui::GuiKind::CraftingTable)
                }
                "open_furnace" => BlockInteraction::OpenGui(crate::gui::GuiKind::Furnace),
                "open_chest" => BlockInteraction::OpenGui(crate::gui::GuiKind::Chest),
                "open_furniture_workbench" => {
                    BlockInteraction::OpenGui(crate::gui::GuiKind::FurnitureWorkbench)
                }
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
                Ok(BlockInteraction::OpenGui(kind))
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
    Translucent,
}

impl RawFlag {
    fn to_flag(self) -> BlockFlags {
        match self {
            RawFlag::Solid => BlockFlags::SOLID,
            RawFlag::Opaque => BlockFlags::OPAQUE,
            RawFlag::AoOccluder => BlockFlags::AO_OCCLUDER,
            RawFlag::Transparent => BlockFlags::TRANSPARENT,
            RawFlag::DirectionalView => BlockFlags::DIRECTIONAL_VIEW,
            RawFlag::Translucent => BlockFlags::TRANSLUCENT,
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

/// The loaded block table: id-indexed defs plus the dense per-id flag and
/// emission copies the mesher/light hot loops read (see `data::flags` /
/// `data::emission`).
pub(super) struct Registry {
    pub defs: &'static [BlockDef],
    /// Session-local shape-kind table (see [`shape_kind`]); every
    /// `BlockDef::shape_kind` indexes it.
    pub shape_kinds: &'static [ShapeKindDef],
    pub flags: [BlockFlags; 256],
    pub emission: [u8; 256],
    /// Dense per-id copy of each block's [`ShapeFamily`] — the hot classifier
    /// the mesher/nav read per cell, one small-array read instead of the
    /// `def()`→`shape_kind`→table double indirection (same rationale as
    /// [`flags`](Self::flags)).
    pub shape_family: [ShapeFamily; 256],
}

/// Load the registry from every `blocks.json` layer (base + mod packs, later
/// packs replacing rows by block — see [`crate::assets::read_layers`]),
/// panicking with a precise message if the table is missing or inconsistent.
pub(super) fn registry() -> Registry {
    // The global name table was built from these same layers, so every row key
    // resolves and every dynamic id is already assigned.
    crate::registry::read_catalog("blocks.json", "block", |texts| {
        parse_layers(texts, crate::registry::names())
    })
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
    // Every row's `shape` interns into this table during `convert`, deduping one
    // shape-kind row per distinct family+params (see [`shape_kind`]).
    let mut interner = shape_kind::ShapeKindInterner::new();
    let defs = crate::registry::resolve_catalog(
        texts,
        |text| serde_json::from_str::<RawFile>(text).map(|f| f.blocks),
        |r| &r.block,
        &names.blocks,
        "block",
        |r, id, _| {
            let key = r.block.clone();
            convert(r, Block(id), names, &mut interner).map_err(|e| format!("block '{key}': {e}"))
        },
    )?;
    let defs: &'static [BlockDef] = Box::leak(defs.into_boxed_slice());
    let shape_kinds: &'static [ShapeKindDef] = Box::leak(interner.into_table().into_boxed_slice());
    validate_stage_chains(defs)?;
    validate_facing_rows(defs)?;
    let mut flags = [BlockFlags::NONE; 256];
    let mut emission = [0u8; 256];
    let mut shape_family = [ShapeFamily::Cube; 256];
    for d in defs {
        flags[d.block.id() as usize] = d.flags;
        emission[d.block.id() as usize] = d.emission;
        shape_family[d.block.id() as usize] = shape_kinds[d.shape_kind.0 as usize].family;
    }
    Ok(Registry {
        defs,
        shape_kinds,
        flags,
        emission,
        shape_family,
    })
}

/// Cross-row sapling checks `convert` can't do alone: every `next_stage`
/// target must itself be a sapling row, and every chain must terminate in a
/// final (`grows_into`) stage — a cycle or a dead end would be a sapling that
/// silently never grows.
fn validate_stage_chains(defs: &[BlockDef]) -> Result<(), String> {
    let name = |d: &BlockDef| format!("{:?}", d.block);
    for d in defs {
        let Some(mut at) = d.next_stage else {
            continue;
        };
        for _ in 0..defs.len() {
            let target = &defs[at.id() as usize];
            if target.behavior.key() != "sapling" {
                return Err(format!(
                    "block {}: next_stage target {:?} does not carry the sapling behaviour",
                    name(d),
                    target.block
                ));
            }
            match target.next_stage {
                Some(next) => at = next,
                None => break, // reached a final (grows_into) stage
            }
        }
        if defs[at.id() as usize].next_stage.is_some() {
            return Err(format!(
                "block {}: its next_stage chain never reaches a final grows_into stage (cycle?)",
                name(d)
            ));
        }
    }
    Ok(())
}

/// Cross-row wall-panel checks `convert` can't do alone: every `facing_rows`
/// target must be a ladder-shaped row whose own `panel_facing` matches the
/// slot it fills, and the declaring row must map its own facing to itself —
/// otherwise placement would commit a panel that doesn't hug the clicked wall.
fn validate_facing_rows(defs: &[BlockDef]) -> Result<(), String> {
    for d in defs {
        let Some(rows) = d.facing_rows else {
            continue;
        };
        for (facing, &target) in [Facing::North, Facing::South, Facing::West, Facing::East]
            .iter()
            .zip(rows.iter())
        {
            let t = &defs[target.id() as usize];
            if t.panel_facing != Some(*facing) {
                return Err(format!(
                    "block {:?}: facing_rows.{} target {:?} does not declare panel_facing '{}'",
                    d.block,
                    facing_name(*facing),
                    target,
                    facing_name(*facing)
                ));
            }
        }
        // Self-consistency: placing this row toward its own facing must keep it.
        let own = d.panel_facing.expect("ladder shape enforced in convert");
        if rows[own.to_u8() as usize] != d.block {
            return Err(format!(
                "block {:?}: facing_rows.{} must name the row itself",
                d.block,
                facing_name(own)
            ));
        }
    }
    Ok(())
}

fn facing_name(f: Facing) -> &'static str {
    match f {
        Facing::North => "north",
        Facing::South => "south",
        Facing::West => "west",
        Facing::East => "east",
    }
}

fn parse_facing(name: &str) -> Result<Facing, String> {
    match name {
        "north" => Ok(Facing::North),
        "south" => Ok(Facing::South),
        "west" => Ok(Facing::West),
        "east" => Ok(Facing::East),
        other => Err(format!("unknown facing '{other}'")),
    }
}

fn convert(
    r: RawBlockDef,
    block: Block,
    names: &ContentNames,
    interner: &mut ShapeKindInterner,
) -> Result<BlockDef, String> {
    let behavior = behavior::by_name(&r.behavior)
        .ok_or_else(|| format!("unknown behavior '{}'", r.behavior))?;
    let interaction = r.interaction.resolve()?;
    let tile = |name: &String| -> Result<Tile, String> {
        Tile::from_name(name).ok_or_else(|| format!("unknown tile '{name}'"))
    };
    let tiles = [tile(&r.tiles[0])?, tile(&r.tiles[1])?, tile(&r.tiles[2])?];
    // Resolve the composable shape kind once; its family/params drive every
    // shape-keyed flag and validation below, and it interns into the table.
    let (family, params, shape_key) = r.shape.resolve()?;
    let mut flags = BlockFlags::NONE;
    for f in &r.flags {
        flags = flags.with(f.to_flag());
    }
    // Derived, not row-listed: the shape class the mesher needs as a dense flag.
    if family == ShapeFamily::Slab {
        flags = flags.with(BlockFlags::SLAB);
    }
    if let Some(h) = params.lowered_height() {
        if !(1..=15).contains(&h) {
            return Err(format!(
                "lowered_cube height {h} out of range (1..=15 texels visible)"
            ));
        }
        // The sunken top means neighbours must keep their faces toward this
        // block — an opaque lowered cube would cull them and open a 1-texel
        // x-ray slit over its top.
        if flags.is_opaque() {
            return Err("a lowered_cube row must not carry the 'opaque' flag".into());
        }
        flags = flags.with(BlockFlags::LOWERED_CUBE);
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
    // Sapling-ness has ONE membership definition: the `sapling` tag (what the
    // block IS, what mods enumerate) and the `sapling` behaviour (what it
    // DOES, the growth) must name the same rows — a tagged-but-inert row
    // would be advertised as growable and never grow; a behaviour row without
    // the tag would be invisible to tag-driven mod policy.
    let is_sapling = behavior.key() == "sapling";
    if is_sapling != tags.contains(&BlockTag::SAPLING) {
        return Err(if is_sapling {
            "a row with the 'sapling' behavior must also list the 'sapling' tag".into()
        } else {
            "the 'sapling' tag requires the 'sapling' behavior (tag and behavior must agree)".into()
        });
    }
    // Growth stages are the block rows themselves: a sapling row is either a
    // growing stage (`next_stage` names its successor) or the final stage
    // (`grows_into` names its tree) — exactly one, and only on sapling rows.
    let next_stage = match &r.next_stage {
        None => None,
        Some(name) => Some(
            names
                .blocks
                .id(name)
                .map(Block)
                .ok_or_else(|| format!("unknown next_stage block '{name}'"))?,
        ),
    };
    let grows_into: Vec<(&'static str, f32)> = match &r.grows_into {
        None => Vec::new(),
        Some(RawGrowsInto::Key(key)) => vec![(resolve_growth_feature(key)?, 1.0)],
        Some(RawGrowsInto::Weighted(choices)) => {
            if choices.is_empty() {
                return Err("grows_into lists no choices".into());
            }
            choices
                .iter()
                .map(|c| {
                    if !(c.weight > 0.0 && c.weight.is_finite()) {
                        return Err(format!(
                            "grows_into '{}' weight must be a positive finite number",
                            c.feature
                        ));
                    }
                    Ok((resolve_growth_feature(&c.feature)?, c.weight as f32))
                })
                .collect::<Result<_, String>>()?
        }
    };
    match (is_sapling, next_stage.is_some(), !grows_into.is_empty()) {
        (false, false, false) | (true, true, false) | (true, false, true) => {}
        (false, ..) => {
            return Err("next_stage/grows_into are sapling-row fields (behavior 'sapling')".into())
        }
        (true, true, true) => {
            return Err(
                "a sapling row is either a growing stage (next_stage) or the final stage \
                 (grows_into), never both"
                    .into(),
            )
        }
        (true, false, false) => {
            return Err(
                "a sapling row must declare next_stage (growing stage) or grows_into (final \
                 stage) — a sapling that names no tree would silently never grow"
                    .into(),
            )
        }
    }
    // A bed is a spawn anchor: the server's bed-spawn bookkeeping (set on the
    // sleep click, verified at respawn, cleared on break) resolves the bed
    // through its MODEL GROUP, and a spawn is only ever SET by a sleep
    // interaction. A `bed`-tagged row that is not a sleepable model block
    // would advertise a spawn anchor the bookkeeping can never set or
    // resolve. The converse is deliberately open: `interaction: "sleep"`
    // without the tag is a sleepable block that anchors no spawn.
    if tags.contains(&BlockTag::BED) {
        if family != ShapeFamily::Model {
            return Err(
                "the 'bed' tag requires a model shape — bed-spawn bookkeeping resolves the \
                 bed through its model group"
                    .into(),
            );
        }
        if interaction != BlockInteraction::Sleep {
            return Err(
                "the 'bed' tag requires interaction 'sleep' — a spawn point is only ever set \
                 by a sleep click, so a non-sleepable bed could never anchor one"
                    .into(),
            );
        }
    }
    // Derived, not row-listed: the physics climb/grip probes need these as
    // dense flags (see `BlockFlags::CLIMBABLE` / `BlockFlags::SLIPPERY`).
    if tags.contains(&BlockTag::CLIMBABLE) {
        flags = flags.with(BlockFlags::CLIMBABLE);
    }
    if tags.contains(&BlockTag::SLIPPERY) {
        flags = flags.with(BlockFlags::SLIPPERY);
    }
    // Row texture vocabulary beyond the plain [top, bottom, side] triple. The
    // front is meaningless without a stored placement facing, which only
    // `directional_view` rows record — refuse the dead data.
    let front = match &r.front {
        None => None,
        Some(name) => {
            if !flags.is_directional_view() {
                return Err("a 'front' tile requires the 'directional_view' flag".into());
            }
            Some(tile(name)?)
        }
    };
    // A wall panel's facing is meaningless off the ladder shape, and a
    // ladder-shaped row without one would mesh/collide/climb some arbitrary
    // default — facing is the shape's identity axis, so both directions of
    // the pairing are load errors (mirroring front ⇔ directional_view).
    let panel_facing = match &r.panel_facing {
        None => {
            if family == ShapeFamily::Ladder {
                return Err(
                    "a ladder-shaped row must declare panel_facing (facing is block identity: \
                     one row per facing)"
                        .into(),
                );
            }
            None
        }
        Some(name) => {
            if family != ShapeFamily::Ladder {
                return Err("panel_facing requires the 'ladder' shape".into());
            }
            Some(parse_facing(name)?)
        }
    };
    let facing_rows = match &r.facing_rows {
        None => None,
        Some(raw) => {
            if family != ShapeFamily::Ladder {
                return Err("facing_rows requires the 'ladder' shape".into());
            }
            let resolve = |name: &String| {
                names
                    .blocks
                    .id(name)
                    .map(Block)
                    .ok_or_else(|| format!("unknown facing_rows block '{name}'"))
            };
            // Facing discriminant order: North, South, West, East.
            let rows: &'static [Block; 4] = Box::leak(Box::new([
                resolve(&raw.north)?,
                resolve(&raw.south)?,
                resolve(&raw.west)?,
                resolve(&raw.east)?,
            ]));
            Some(rows)
        }
    };
    let side_overlay = match &r.side_overlay {
        None => None,
        Some(raw) => Some(definition::SideOverlay {
            base: tile(&raw.base)?,
            overlay: tile(&raw.overlay)?,
        }),
    };
    let covered_side = match &r.covered_side {
        None => None,
        Some(name) => Some(tile(name)?),
    };
    let particle_emitter: Option<&'static [ParticleEmitter]> = match &r.particle_emitter {
        None => None,
        Some(RawEmitterRef::Key(key)) => {
            let bundle = crate::particle_emitters::by_key(key)
                .ok_or_else(|| format!("unknown particle_emitter bundle '{key}'"))?;
            if bundle.burst.is_some() {
                return Err(format!(
                    "particle_emitter '{key}' is a one-shot burst bundle; blocks show looping \
                     bundles only"
                ));
            }
            Some(bundle.rows)
        }
        Some(RawEmitterRef::Inline(row)) => {
            validate_particle_emitter(row)?;
            Some(Box::leak(Box::new([*row])))
        }
    };
    let shape_kind = interner.intern(family, params, shape_key)?;
    Ok(BlockDef {
        block,
        flags,
        tags: leak(tags),
        behavior,
        interaction,
        shape_kind,
        collision: leak(r.collision),
        emission: r.emission,
        particle_emitter,
        tiles,
        front,
        side_overlay,
        covered_side,
        material: r.material,
        harvest_tier: r.harvest_tier,
        hardness: r.hardness as f32,
        drop: DropSpec { drops: leak(drops) },
        next_stage,
        grows_into: leak(grows_into),
        panel_facing,
        facing_rows,
    })
}

/// Validate one `grows_into` feature key against the loaded feature registry
/// and intern it. Unknown keys fail the load — a final sapling stage naming a
/// missing tree must never fall back to some default species.
fn resolve_growth_feature(key: &str) -> Result<&'static str, String> {
    if crate::worldgen::data::features::by_name(key).is_none() {
        return Err(format!("grows_into names unknown worldgen feature '{key}'"));
    }
    Ok(String::leak(key.to_owned()))
}

/// Shared strict validation for one particle-emitter row — used by block rows
/// (`blocks.json` `particle_emitter`) and mob rows (`mobs.json`
/// `particle_emitters` entries), which speak the same schema.
pub(crate) fn validate_particle_emitter(e: &ParticleEmitter) -> Result<(), String> {
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
        ("spiral", e.spiral.as_slice()),
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
    let color_stops: &[[f32; 3]] = match (&e.color, &e.color_ramp) {
        (Some(_), Some(_)) => {
            return Err("particle_emitter declares both color and color_ramp — pick one".into())
        }
        (None, None) => {
            return Err("particle_emitter needs either color or color_ramp".into());
        }
        (Some(endpoints), None) => endpoints.as_slice(),
        (None, Some(ramp)) => ramp.stops(),
    };
    for (stop, color) in color_stops.iter().enumerate() {
        for (channel, value) in color.iter().enumerate() {
            finite(&format!("color[{stop}][{channel}]"), *value)?;
            if !(0.0..=1.0).contains(value) {
                return Err("particle_emitter color channels must be in 0..=1".into());
            }
        }
    }
    for (label, power) in [
        ("fade_power", e.fade_power),
        ("shrink_power", e.shrink_power),
    ] {
        finite(label, power)?;
        if !(0.25..=8.0).contains(&power) {
            return Err(format!("particle_emitter.{label} must be in 0.25..=8"));
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
    if e.spiral[0] < 0.0 {
        return Err("particle_emitter.spiral radius must be >= 0".into());
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

    /// The `bed` tag is the spawn-anchor identity the server's bed bookkeeping
    /// keys on; it resolves the bed through its model group and only a sleep
    /// click ever sets a spawn — so a tagged row that is not a sleepable model
    /// block must fail the load instead of silently never anchoring.
    #[test]
    fn bed_tagged_rows_must_be_sleepable_model_blocks() {
        let (base, _) =
            crate::assets::read_base_text("blocks.json").expect("assets/blocks.json must ship");
        // A bed-tagged CUBE row: the bookkeeping could never resolve its group.
        let cube = r#"{ "blocks": [ { "block": "petramond:stone", "shape": "cube", "flags": ["solid", "opaque", "ao_occluder"], "tags": ["bed"], "behavior": "inert", "interaction": "sleep", "collision": [{"min": [0, 0, 0], "max": [1, 1, 1]}], "emission": 0, "tiles": ["stone", "stone", "stone"], "material": "stone", "harvest_tier": 1, "hardness": 1, "drops": [] } ] }"#;
        let err = parse_test_layers(&[&base, cube])
            .err()
            .expect("bed tag on a cube refused");
        assert!(err.contains("model shape"), "{err}");
        // A bed-tagged model row WITHOUT the sleep interaction: no click could
        // ever set the spawn it advertises.
        let unsleepable = r#"{ "blocks": [ { "block": "petramond:bed", "shape": {"model": "petramond:bed"}, "flags": ["solid", "directional_view"], "tags": ["bed"], "behavior": "inert", "interaction": "none", "collision": [], "emission": 0, "tiles": ["oak_planks", "oak_planks", "oak_planks"], "material": "wood", "harvest_tier": 0, "hardness": 1, "drops": [] } ] }"#;
        let err = parse_test_layers(&[&base, unsleepable])
            .err()
            .expect("unsleepable bed tag refused");
        assert!(err.contains("interaction 'sleep'"), "{err}");
        // The open converse: `interaction: "sleep"` WITHOUT the tag is a
        // sleepable block that anchors no spawn — legal.
        let sleep_only = r#"{ "blocks": [ { "block": "petramond:bed", "shape": {"model": "petramond:bed"}, "flags": ["solid", "directional_view"], "tags": [], "behavior": "inert", "interaction": "sleep", "collision": [], "emission": 0, "tiles": ["oak_planks", "oak_planks", "oak_planks"], "material": "wood", "harvest_tier": 0, "hardness": 1, "drops": [] } ] }"#;
        parse_test_layers(&[&base, sleep_only]).expect("sleep without the bed tag loads");
    }

    #[test]
    fn pack_layer_overrides_rows_by_block() {
        let (base, _) =
            crate::assets::read_base_text("blocks.json").expect("assets/blocks.json must ship");
        let layer = r#"{ "blocks": [ { "block": "petramond:stone", "shape": "cube", "flags": ["solid", "opaque", "ao_occluder"], "tags": ["terrain"], "behavior": "inert", "interaction": "none", "collision": [{"min": [0, 0, 0], "max": [1, 1, 1]}], "emission": 0, "tiles": ["stone", "stone", "stone"], "material": "stone", "harvest_tier": 1, "hardness": 99, "drops": [] } ] }"#;
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
        let layer = r#"{ "blocks": [ { "block": "mymod:glowrock", "shape": "cube", "flags": ["solid", "opaque", "ao_occluder"], "tags": [], "behavior": "inert", "interaction": "none", "collision": [{"min": [0, 0, 0], "max": [1, 1, 1]}], "emission": 28, "tiles": ["stone", "stone", "stone"], "material": "stone", "harvest_tier": 1, "hardness": 2, "drops": [{"item": "petramond:cobblestone", "min": 1, "max": 1, "chance": 1.0}] } ] }"#;
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

    /// Layer 2: a `{"custom": {...}}` shape parameterizes an existing family
    /// (fence/pane) from JSON — no WASM. The loader resolves it to a
    /// `Connection` shape kind with the declared post dimensions + rule, and
    /// rejects out-of-range / unknown / unsupported combinations.
    #[test]
    fn custom_connection_shapes_load_resolve_and_validate() {
        use crate::block::ConnectionRule;
        let (base, _) =
            crate::assets::read_base_text("blocks.json").expect("assets/blocks.json must ship");
        let row = |name: &str, shape: &str| {
            format!(
                r#"{{ "blocks": [ {{ "block": "{name}", "shape": {shape}, "flags": [], "tags": [], "behavior": "inert", "interaction": "none", "collision": [], "emission": 0, "tiles": ["stone", "stone", "stone"], "material": "stone", "harvest_tier": 1, "hardness": 2, "drops": [] }} ] }}"#
            )
        };
        let engine = crate::block::ENGINE_BLOCK_NAMES.len();

        // A fence-family wall with a thick centred post resolves to a Connection
        // kind: offset (16-8)/2 = 4, so post 4/16..12/16, engine fence rule.
        let wall = row("mymod:stone_wall", r#"{"custom": {"family": "fence", "post_thickness": 8}}"#);
        let reg = parse_test_layers(&[&base, &wall]).expect("custom fence wall loads");
        let def = &reg.defs[engine];
        let sk = &reg.shape_kinds[def.shape_kind.0 as usize];
        assert_eq!(sk.family, ShapeFamily::Fence);
        let c = sk.params.connection().expect("connection params");
        assert_eq!(c.post_lo, 4.0 / 16.0);
        assert_eq!(c.post_hi, 12.0 / 16.0);
        assert_eq!(c.rule, ConnectionRule::OpaqueOrSame);
        // The box table's bare-post entry matches the declared post.
        let post = crate::connect::boxes_for_mask(c.boxes, 0)[0];
        assert_eq!(post.min, [4.0 / 16.0, 0.0, 4.0 / 16.0]);

        // A pane-family bar with an explicit rule resolves to that rule.
        let bar = row(
            "mymod:iron_bars",
            r#"{"custom": {"family": "pane", "post_thickness": 2, "connection_rule": "same_family_only"}}"#,
        );
        let reg = parse_test_layers(&[&base, &bar]).expect("custom pane bar loads");
        let sk = &reg.shape_kinds[reg.defs[engine].shape_kind.0 as usize];
        assert_eq!(sk.family, ShapeFamily::Pane);
        assert_eq!(sk.params.connection().unwrap().rule, ConnectionRule::SameOnly);

        // Validation failures.
        for (shape, needle) in [
            (r#"{"custom": {"family": "bogus"}}"#, "unknown custom shape family"),
            (r#"{"custom": {"family": "fence", "post_thickness": 0}}"#, "post_thickness"),
            (
                r#"{"custom": {"family": "fence", "post_thickness": 10, "post_offset": 10}}"#,
                "exceeds 16",
            ),
            (r#"{"custom": {"family": "fence", "connection_rule": "nope"}}"#, "unknown connection_rule"),
            (r#"{"custom": {"family": "fence", "item_form": "nope"}}"#, "unknown item_form"),
            (r#"{"custom": {"family": "pane", "item_form": "segment"}}"#, "requires the 'fence' family"),
        ] {
            let layer = row("mymod:bad", shape);
            let err = parse_test_layers(&[&base, &layer])
                .err()
                .unwrap_or_else(|| panic!("{shape} must fail the load"));
            assert!(err.contains(needle), "{shape}: {err}");
        }
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
    fn particle_emitter_rows_take_exactly_one_color_form() {
        let (base, _) =
            crate::assets::read_base_text("blocks.json").expect("assets/blocks.json must ship");
        let row = |emitter: &str| {
            format!(
                r#"{{ "blocks": [ {{ "block": "mymod:spark", "shape": "cube", "flags": [], "tags": [], "behavior": "inert", "interaction": "none", "collision": [], "emission": 0, "particle_emitter": {{ "rate": 2.0, "lifetime": [0.2, 0.5], "size": [0.02, 0.05], "alpha": [0.2, 0.6]{emitter} }}, "tiles": ["stone", "stone", "stone"], "material": "stone", "harvest_tier": 0, "hardness": 1, "drops": [] }} ] }}"#
            )
        };

        let ramp = row(
            r#", "color_ramp": [[1.0, 1.0, 0.9], [1.0, 0.5, 0.1], [0.1, 0.1, 0.1]], "fade_power": 1.0"#,
        );
        parse_test_layers(&[&base, ramp.as_str()]).expect("a ramp row loads");

        for (emitter, why) in [
            ("".to_owned(), "neither color form"),
            (
                row(r#", "color": [[1, 1, 1], [1, 1, 1]], "color_ramp": [[1, 1, 1], [0, 0, 0]]"#),
                "both color forms",
            ),
            (row(r#", "color_ramp": [[1, 1, 1]]"#), "a one-stop ramp"),
            (
                row(r#", "color": [[1, 1, 1], [1, 1, 1]], "fade_power": 100.0"#),
                "an out-of-range fade_power",
            ),
        ] {
            let layer = if emitter.is_empty() { row("") } else { emitter };
            assert!(
                parse_test_layers(&[&base, layer.as_str()]).is_err(),
                "{why} must fail the load"
            );
        }
    }

    /// Sapling-ness is ONE membership (D6) and the stage chain is validated
    /// data (E3): tag ⇔ behavior must agree, a sapling row carries exactly one
    /// of `next_stage`/`grows_into`, `grows_into` must name a real worldgen
    /// feature, and a chain must terminate in a final stage.
    #[test]
    fn sapling_rows_validate_tag_behavior_and_stage_chain() {
        let (base, _) =
            crate::assets::read_base_text("blocks.json").expect("assets/blocks.json must ship");
        let row = |name: &str, tags: &str, behavior: &str, growth: &str| {
            format!(
                r#"{{ "blocks": [ {{ "block": "{name}", {growth} "shape": "cross", "flags": ["transparent"], "tags": [{tags}], "behavior": "{behavior}", "interaction": "none", "collision": [], "emission": 0, "tiles": ["oak_sapling", "oak_sapling", "oak_sapling"], "material": "plant", "harvest_tier": 0, "hardness": 0, "drops": [] }} ] }}"#
            )
        };
        let sapling_tags = r#""fragile", "roots_in_soil", "sapling""#;

        // A valid pack sapling: final stage, weighted grows_into.
        let good = row(
            "mymod:sap",
            sapling_tags,
            "sapling",
            r#""grows_into": [{"feature": "petramond:oak_big", "weight": 1}, {"feature": "petramond:oak_small"}],"#,
        );
        parse_test_layers(&[&base, &good]).expect("a valid final-stage sapling row loads");
        // ... and a growing stage chaining into an engine row.
        let chained = row(
            "mymod:sap",
            sapling_tags,
            "sapling",
            r#""next_stage": "petramond:oak_sapling_1","#,
        );
        parse_test_layers(&[&base, &chained]).expect("a valid growing-stage sapling row loads");

        for (layer, why, needle) in [
            (
                row(
                    "mymod:sap",
                    r#""fragile""#,
                    "sapling",
                    r#""grows_into": "petramond:spruce","#,
                ),
                "behavior without the tag",
                "tag",
            ),
            (
                row("mymod:sap", sapling_tags, "fragile", ""),
                "tag without the behavior",
                "behavior",
            ),
            (
                row("mymod:sap", sapling_tags, "sapling", ""),
                "a sapling row with neither stage field",
                "next_stage",
            ),
            (
                row(
                    "mymod:sap",
                    sapling_tags,
                    "sapling",
                    r#""next_stage": "petramond:oak_sapling_1", "grows_into": "petramond:spruce","#,
                ),
                "both stage fields at once",
                "never both",
            ),
            (
                row(
                    "mymod:sap",
                    sapling_tags,
                    "sapling",
                    r#""grows_into": "petramond:not_a_feature","#,
                ),
                "an unknown grows_into feature",
                "unknown worldgen feature",
            ),
            (
                row(
                    "mymod:notsap",
                    r#""fragile""#,
                    "fragile",
                    r#""next_stage": "petramond:oak_sapling","#,
                ),
                "next_stage on a non-sapling row",
                "sapling-row fields",
            ),
            (
                row(
                    "mymod:sap",
                    sapling_tags,
                    "sapling",
                    r#""next_stage": "mymod:sap","#,
                ),
                "a self-referential chain that never reaches a final stage",
                "never reaches",
            ),
            (
                row(
                    "mymod:sap",
                    sapling_tags,
                    "sapling",
                    r#""next_stage": "petramond:stone","#,
                ),
                "a chain leaving the sapling rows",
                "sapling behaviour",
            ),
        ] {
            let err = parse_test_layers(&[&base, &layer])
                .err()
                .unwrap_or_else(|| panic!("{why} must fail the load"));
            assert!(err.contains(needle), "{why}: {err}");
        }
    }

    /// Wall-panel facing is block identity (one ladder row per facing), so the
    /// load enforces the pairing both ways and cross-validates the placeable
    /// row's `facing_rows` map — a mismatched sibling would place a panel that
    /// doesn't hug the clicked wall.
    #[test]
    fn wall_panel_rows_validate_facing_identity_and_sibling_map() {
        let (base, _) =
            crate::assets::read_base_text("blocks.json").expect("assets/blocks.json must ship");
        let row = |name: &str, shape: &str, extra: &str| {
            format!(
                r#"{{ "blocks": [ {{ "block": "{name}", "shape": "{shape}", {extra} "flags": ["transparent"], "tags": ["fragile", "climbable"], "behavior": "fragile", "interaction": "none", "collision": [], "emission": 0, "tiles": ["ladder", "ladder", "ladder"], "material": "wood", "harvest_tier": 0, "hardness": 0.4, "drops": [] }} ] }}"#
            )
        };

        // A pack's single-facing panel row loads (no sibling map needed —
        // placement then keeps its declared facing).
        let single = row("mymod:vine_panel", "ladder", r#""panel_facing": "south","#);
        parse_test_layers(&[&base, &single]).expect("a single-facing wall panel loads");

        for (layer, why, needle) in [
            (
                row("mymod:vine_panel", "ladder", ""),
                "a ladder-shaped row without panel_facing",
                "panel_facing",
            ),
            (
                row("mymod:vine_panel", "cross", r#""panel_facing": "south","#),
                "panel_facing off the ladder shape",
                "'ladder' shape",
            ),
            (
                row(
                    "mymod:vine_panel",
                    "ladder",
                    r#""panel_facing": "sideways","#,
                ),
                "an unknown facing name",
                "unknown facing",
            ),
            (
                // The engine ladder's map re-pointed at a wrong-facing sibling.
                row(
                    "petramond:ladder",
                    "ladder",
                    r#""panel_facing": "north", "facing_rows": {"north": "petramond:ladder", "south": "petramond:ladder_south", "west": "petramond:ladder_west", "east": "petramond:ladder_south"},"#,
                ),
                "a facing_rows slot naming a wrong-facing row",
                "facing_rows.east",
            ),
        ] {
            let err = parse_test_layers(&[&base, &layer])
                .err()
                .unwrap_or_else(|| panic!("{why} must fail the load"));
            assert!(err.contains(needle), "{why}: {err}");
        }

        // Every slot's facing matches, but the row maps its OWN facing to a
        // different row: placing it toward its own facing would swap blocks.
        let stranger = row("mymod:north_panel", "ladder", r#""panel_facing": "north","#);
        let bad_self = row(
            "petramond:ladder",
            "ladder",
            r#""panel_facing": "north", "facing_rows": {"north": "mymod:north_panel", "south": "petramond:ladder_south", "west": "petramond:ladder_west", "east": "petramond:ladder_east"},"#,
        );
        let err = parse_test_layers(&[&base, &stranger, &bad_self])
            .err()
            .expect("a non-self own-facing slot must fail the load");
        assert!(err.contains("the row itself"), "{err}");
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

    /// `interaction: {"open_gui": "mod:kind"}` resolves to
    /// `OpenGui` with a registered kind; a bare (un-namespaced) open_gui
    /// key and an unknown named interaction are load errors.
    #[test]
    fn open_gui_interaction_parses_namespaced_and_rejects_bare() {
        let (base, _) =
            crate::assets::read_base_text("blocks.json").expect("assets/blocks.json must ship");
        let layer = r#"{ "blocks": [ { "block": "guimod:opener", "shape": "cube", "flags": ["solid", "opaque", "ao_occluder"], "tags": [], "behavior": "inert", "interaction": {"open_gui": "guimod:panel"}, "collision": [{"min": [0, 0, 0], "max": [1, 1, 1]}], "emission": 0, "tiles": ["stone", "stone", "stone"], "material": "stone", "harvest_tier": 1, "hardness": 2, "drops": [] } ] }"#;
        let reg = parse_test_layers(&[&base, layer]).expect("open_gui row loads");
        let def = &reg.defs[crate::block::ENGINE_BLOCK_NAMES.len()];
        let BlockInteraction::OpenGui(kind) = def.interaction else {
            panic!("expected OpenGui, got {:?}", def.interaction);
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
        let partial = r#"{ "blocks": [ { "block": "petramond:air", "shape": "cube", "flags": [], "tags": [], "behavior": "inert", "interaction": "none", "collision": [], "emission": 0, "tiles": ["dirt", "dirt", "dirt"], "material": "none", "harvest_tier": 0, "hardness": -1, "drops": [] } ] }"#;
        assert!(parse(partial).err().unwrap().contains("missing row"));
        // Unknown behavior name (the full base table with one row broken).
        let (base, _) =
            crate::assets::read_base_text("blocks.json").expect("assets/blocks.json must ship");
        let bad_behavior = r#"{ "blocks": [ { "block": "petramond:air", "shape": "cube", "flags": [], "tags": [], "behavior": "bogus", "interaction": "none", "collision": [], "emission": 0, "tiles": ["dirt", "dirt", "dirt"], "material": "none", "harvest_tier": 0, "hardness": -1, "drops": [] } ] }"#;
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
