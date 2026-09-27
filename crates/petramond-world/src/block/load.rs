use serde::{Deserialize, Serialize};

use crate::facing::Facing;
use crate::item::{Drop, DropSpec, ItemType};
use crate::registry::ContentNames;
use crate::tile::Tile;

use super::definition::{
    self, BlockDef, BlockFlags, BlockMaterial, ParticleEmitter, RootsFace, SupportDir,
};
use super::shape_kind::{
    self, MeshEmitter, RawShape, RowFacts, ShapeKindDef, ShapeKindInterner, ShapeSim,
};
use super::{behavior, Aabb, Block, BlockInteraction, BlockTag};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawBlockDef {
    pub block: String,
    pub shape: RawShape,
    pub flags: Vec<RawFlag>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contained_fluid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub melts_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fluid: Option<crate::fluid::load::RawFluid>,
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub corners: bool,
    pub behavior: String,
    pub interaction: RawInteraction,
    pub collision: Vec<Aabb>,
    pub emission: u8,
    #[serde(default = "white_light", skip_serializing_if = "is_white_light")]
    pub light_color: [f64; 3],
    #[serde(default)]
    pub particle_emitter: Option<RawEmitterRef>,
    pub tiles: [String; 3],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uv_rotation: Option<std::collections::BTreeMap<String, u16>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub front: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side_overlay: Option<RawSideOverlay>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub covered_side: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow_tile: Option<String>,
    pub material: BlockMaterial,
    pub hardness: f64,
    pub drops: Vec<RawDrop>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_stage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grows_into: Option<RawGrowsInto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel_facing: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facing_rows: Option<RawFacingRows>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flipped: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animated_model: Option<String>,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub data: serde_json::Map<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "SupportDir::is_default")]
    pub support: SupportDir,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roots_on: Vec<String>,
    #[serde(default, skip_serializing_if = "RootsFace::is_default")]
    pub roots_face: RootsFace,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawHarvest {
    pub tier: u8,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum RawConstruction {
    Item(String),
    Form(String),
    Unsupported(String),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawFacingRows {
    pub north: String,
    pub south: String,
    pub west: String,
    pub east: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawSideOverlay {
    pub base: String,
    pub overlay: String,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum RawGrowsInto {
    Key(String),
    Weighted(Vec<RawGrowthChoice>),
}

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

fn white_light() -> [f64; 3] {
    [1.0, 1.0, 1.0]
}

fn is_white_light(c: &[f64; 3]) -> bool {
    *c == white_light()
}

fn resolve_emission_rgb(emission: u8, color: [f64; 3]) -> Result<[u8; 3], String> {
    if emission > crate::chunk::SKY_FULL {
        return Err(format!(
            "emission {emission} exceeds the maximum light level {} — brighter than full daylight",
            crate::chunk::SKY_FULL
        ));
    }
    if is_white_light(&color) {
        return Ok([emission; 3]);
    }
    if emission == 0 {
        return Err(
            "light_color on a row that emits no light (emission 0) — put it on the LIT row"
                .to_string(),
        );
    }
    for (axis, c) in ["r", "g", "b"].iter().zip(color) {
        if !(c.is_finite() && (0.0..=1.0).contains(&c)) {
            return Err(format!("light_color.{axis} must be within 0.0..=1.0"));
        }
    }
    let rgb: [u8; 3] = std::array::from_fn(|i| (emission as f64 * color[i]).round() as u8);
    if rgb == [0; 3] {
        return Err(format!(
            "light_color scales emission {emission} to zero on every channel — the emitter \
             scan would count a cell that floods no light"
        ));
    }
    Ok(rgb)
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum RawEmitterRef {
    Key(String),
    Inline(Box<ParticleEmitter>),
}

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
                "open_crafting_table" => {
                    BlockInteraction::OpenGui(crate::gui_state::GuiKind::CraftingTable)
                }
                "open_furnace" => BlockInteraction::OpenGui(crate::gui_state::GuiKind::Furnace),
                "open_chest" => BlockInteraction::OpenGui(crate::gui_state::GuiKind::Chest),
                "open_furniture_workbench" => {
                    BlockInteraction::OpenGui(crate::gui_state::GuiKind::FurnitureWorkbench)
                }
                "open_chiseling_station" => {
                    BlockInteraction::OpenGui(crate::gui_state::GuiKind::ChiselingStation)
                }
                "toggle_door" => BlockInteraction::ToggleDoor,
                "toggle_trapdoor" => BlockInteraction::ToggleTrapdoor,
                "sleep" => BlockInteraction::Sleep,
                other => return Err(format!("unknown interaction '{other}'")),
            }),
            RawInteraction::OpenGui { open_gui } => {
                if !crate::registry::is_namespaced(open_gui) {
                    return Err(format!(
                        "open_gui '{open_gui}' must be a namespaced 'mod_id:name' GUI kind"
                    ));
                }
                let kind = crate::gui_state::intern_kind(open_gui)
                    .ok_or_else(|| format!("cannot register gui kind '{open_gui}'"))?;
                Ok(BlockInteraction::OpenGui(kind))
            }
        }
    }
}

#[derive(Copy, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum RawFlag {
    Invisible,
    Fluid,
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
            RawFlag::Invisible => BlockFlags::INVISIBLE,
            RawFlag::Fluid => BlockFlags::FLUID,
            RawFlag::Solid => BlockFlags::SOLID,
            RawFlag::Opaque => BlockFlags::OPAQUE,
            RawFlag::AoOccluder => BlockFlags::AO_OCCLUDER,
            RawFlag::Transparent => BlockFlags::TRANSPARENT,
            RawFlag::DirectionalView => BlockFlags::DIRECTIONAL_VIEW,
            RawFlag::Translucent => BlockFlags::TRANSLUCENT,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawDrop {
    pub item: String,
    pub min: u8,
    pub max: u8,
    pub chance: f64,
}

pub(crate) struct Registry {
    pub defs: &'static [BlockDef],
    pub all: Box<[Block]>,
    pub shape_kinds: &'static [ShapeKindDef],
    pub flags: Box<[BlockFlags]>,
    pub emission: Box<[u8]>,
    pub emission_rgb: Box<[[u8; 3]]>,
    pub shape_refines: Box<[bool]>,
    pub shape_custom: Box<[bool]>,
    pub tag_bits: Box<[u128]>,
}

pub(super) const TAG_BITS_MAX: u8 = 127;

pub(crate) fn load_registry(
    packs: &crate::assets::PackSet,
    names: &ContentNames,
) -> Result<Registry, String> {
    crate::registry::read_catalog(packs, "blocks.json", "block", |texts| {
        parse_layers(texts, names)
    })
}

#[cfg(test)]
pub(super) fn parse(text: &str) -> Result<Registry, String> {
    parse_test_layers(&[text])
}

#[cfg(test)]
pub(super) fn parse_test_layers(texts: &[&str]) -> Result<Registry, String> {
    let (items, _) =
        crate::assets::read_base_text("items.json").expect("assets/items.json must ship");
    let names = crate::registry::build_names(texts, &[&items])?;
    parse_layers(texts, &names)
}

pub(super) fn parse_layers(texts: &[&str], names: &ContentNames) -> Result<Registry, String> {
    let mut interner = shape_kind::ShapeKindInterner::new();
    let patches = std::cell::RefCell::new(Vec::new());
    let defs = crate::registry::resolve_catalog(
        texts,
        |text| {
            crate::registry::parse_rows_with_patches(
                text,
                "blocks",
                "block",
                &mut patches.borrow_mut(),
            )
        },
        |r: &RawBlockDef| &r.block,
        &names.blocks,
        "block",
        |r, id, _| {
            let key = r.block.clone();
            convert(r, Block(id), names, &mut interner, &patches.borrow())
                .map_err(|e| format!("block '{key}': {e}"))
        },
    )?;
    for p in patches.borrow().iter() {
        if names.blocks.id(&p.patch).is_none() {
            return Err(format!("data patch targets unknown block '{}'", p.patch));
        }
    }
    let defs: &'static [BlockDef] = Box::leak(defs.into_boxed_slice());
    let shape_kinds: &'static [ShapeKindDef] = Box::leak(interner.into_table().into_boxed_slice());
    for row in defs {
        if let Some(quench) = row.fluid.and_then(|f| f.quench) {
            let name = names.blocks.name(row.block.id()).unwrap();
            let by = defs[quench.by.id() as usize].fluid;
            if quench.by == row.block
                || by.is_none()
                || defs[quench.result.id() as usize].fluid.is_some()
            {
                return Err(format!(
                    "block '{name}' requires quench.by to be another fluid \
                     and quench.result to be nonfluid"
                ));
            }
            if by.and_then(|f| f.quench).is_some_and(|q| q.by == row.block) {
                return Err(format!(
                    "block '{name}' and '{}' quench each other; \
                     only one fluid of a pair may declare the quench",
                    names.blocks.name(quench.by.id()).unwrap()
                ));
            }
        }
        if let Some(fluid) = row.contained_fluid {
            let target = &defs[fluid.id() as usize];
            if !target.flags.fluid() {
                return Err(format!(
                    "block '{}' contains a non-fluid block",
                    names.blocks.name(row.block.id()).unwrap()
                ));
            }
        }
        if let Some(residue) = row.melts_to {
            if !defs[residue.id() as usize].flags.fluid() || row.flags.fluid() {
                return Err(format!(
                    "block '{}' melts_to must name a fluid block from a non-fluid row",
                    names.blocks.name(row.block.id()).unwrap()
                ));
            }
        }
    }
    validate_stage_chains(defs)?;
    validate_facing_rows(defs)?;
    validate_flipped_rows(defs, shape_kinds)?;
    validate_roots_on(defs)?;
    let n = defs.len();
    let mut flags = vec![BlockFlags::NONE; n].into_boxed_slice();
    let mut emission = vec![0u8; n].into_boxed_slice();
    let mut emission_rgb = vec![[0u8; 3]; n].into_boxed_slice();
    let mut shape_refines = vec![false; n].into_boxed_slice();
    let mut shape_custom = vec![false; n].into_boxed_slice();
    let mut tag_bits = vec![0u128; n].into_boxed_slice();
    for d in defs {
        for t in d.tags {
            if t.id() <= TAG_BITS_MAX {
                tag_bits[d.block.id() as usize] |= 1u128 << t.id();
            }
        }
        flags[d.block.id() as usize] = d.flags;
        emission[d.block.id() as usize] = d.emission;
        emission_rgb[d.block.id() as usize] = d.emission_rgb;
        let kind = &shape_kinds[d.shape_kind.0 as usize];
        shape_refines[d.block.id() as usize] = kind.refines;
        shape_custom[d.block.id() as usize] = kind.params.custom().is_some();
    }
    Ok(Registry {
        defs,
        all: (0..n).map(|id| Block(id as u16)).collect(),
        shape_kinds,
        flags,
        emission,
        emission_rgb,
        shape_refines,
        shape_custom,
        tag_bits,
    })
}

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
                None => break,
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

fn validate_roots_on(defs: &[BlockDef]) -> Result<(), String> {
    for d in defs {
        for tag in d.roots_on {
            if !defs.iter().any(|g| g.tags.contains(tag)) {
                return Err(format!(
                    "block {:?}: roots_on names tag {tag:?}, which no loaded block carries",
                    d.block
                ));
            }
        }
    }
    Ok(())
}

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

fn validate_flipped_rows(defs: &[BlockDef], kinds: &[ShapeKindDef]) -> Result<(), String> {
    let root_of = |d: &BlockDef| {
        kinds[d.shape_kind.0 as usize]
            .params
            .box_set()
            .and_then(|b| b.run())
            .map(|r| r.root)
    };
    for d in defs {
        let Some(target) = d.flipped_row else {
            continue;
        };
        let own = root_of(d).expect("run shape enforced in convert");
        let t = &defs[target.id() as usize];
        if root_of(t) != Some(own.opposite()) {
            return Err(format!(
                "block {:?}: flipped target {:?} is not a run rooted '{}'",
                d.block,
                target,
                own.opposite().name()
            ));
        }
        if t.flipped_row != Some(d.block) {
            return Err(format!(
                "block {:?}: flipped target {:?} must name it back",
                d.block, target
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

fn resolve_uv_turns(r: &RawBlockDef, sim: &dyn ShapeSim) -> Result<[u8; 3], String> {
    let Some(map) = &r.uv_rotation else {
        return Ok([0; 3]);
    };
    if !sim.accepts_row_uv_rotation() {
        return Err(
            "'uv_rotation' rotates the row's [top, bottom, side] tiles and only \
             applies to the cube/stair/slab shapes (a box set rotates per box, \
             via 'uv_rotation' on each box)"
                .into(),
        );
    }
    let mut turns = [0u8; 3];
    for (name, deg) in map {
        let slot = match name.as_str() {
            "top" => 0,
            "bottom" => 1,
            "side" => 2,
            other => {
                return Err(format!(
                    "unknown uv_rotation slot '{other}' (expected top, bottom or side)"
                ))
            }
        };
        if *deg > 270 || deg % 90 != 0 {
            return Err(format!(
                "uv_rotation '{name}' {deg} must be 0, 90, 180 or 270"
            ));
        }
        turns[slot] = (deg / 90) as u8;
    }
    Ok(turns)
}

fn convert(
    r: RawBlockDef,
    block: Block,
    names: &ContentNames,
    interner: &mut ShapeKindInterner,
    patches: &[crate::registry::RawDataPatch],
) -> Result<BlockDef, String> {
    let behavior = behavior::by_name(&r.behavior)
        .ok_or_else(|| format!("unknown behavior '{}'", r.behavior))?;
    let interaction = r.interaction.resolve()?;
    let tile = |name: &String| -> Result<Tile, String> {
        Tile::from_name(name).ok_or_else(|| format!("unknown tile '{name}'"))
    };
    let tiles = [tile(&r.tiles[0])?, tile(&r.tiles[1])?, tile(&r.tiles[2])?];
    let (family, params, shape_key) = r.shape.resolve(r.corners)?;
    let (sim, render, _) = shape_kind::families::singletons(family);
    let uv_turns = resolve_uv_turns(&r, sim)?;
    let mut flags = BlockFlags::NONE;
    for f in &r.flags {
        flags = flags.with(f.to_flag());
    }
    let contained_fluid = r
        .contained_fluid
        .as_ref()
        .map(|name| {
            names
                .blocks
                .id(name)
                .map(Block)
                .ok_or_else(|| format!("unknown contained fluid '{name}'"))
        })
        .transpose()?;
    let melts_to = r
        .melts_to
        .as_ref()
        .map(|name| {
            names
                .blocks
                .id(name)
                .map(Block)
                .ok_or_else(|| format!("unknown melts_to block '{name}'"))
        })
        .transpose()?;
    if contained_fluid.is_some() {
        if flags.is_opaque() || flags.fluid() {
            return Err("contained_fluid requires a nonopaque, nonfluid host block".into());
        }
        flags = flags.with(BlockFlags::CONTAINS_FLUID);
    }
    if flags.fluid() && (!sim.hosts_fluid() || flags.is_opaque() || flags.is_solid()) {
        return Err("fluid requires a nonopaque, nonsolid cube".into());
    }
    flags = flags.with(sim.row_flags());
    if render.mesh_emitter(&params) == MeshEmitter::Boxes {
        flags = flags.with(BlockFlags::BOX_SHAPE);
    }
    sim.validate_row(
        &params,
        &RowFacts {
            flags,
            corners: r.corners,
            authored_collision: !r.collision.is_empty(),
        },
    )?;
    let animated_model = r
        .animated_model
        .as_deref()
        .map(|key| {
            crate::animated_model::by_key(key)
                .ok_or_else(|| format!("unknown animated_model '{key}'"))
        })
        .transpose()?;
    let draws_itself = render.mesh_emitter(&params) != MeshEmitter::Nothing;
    match animated_model {
        None if !draws_itself => {
            return Err(
                "this shape draws nothing itself: the row must declare an animated_model".into(),
            )
        }
        Some(_) if draws_itself && !flags.is_directional_view() => {
            return Err(
                "an animated_model on this shape requires the 'directional_view' flag (the \
                 model is posed by, and found through, the stored placement front)"
                    .into(),
            )
        }
        _ => {}
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
    let is_sapling = behavior.key() == "sapling";
    if is_sapling != tags.contains(&BlockTag::SAPLING) {
        return Err(if is_sapling {
            "a row with the 'sapling' behavior must also list the 'sapling' tag".into()
        } else {
            "the 'sapling' tag requires the 'sapling' behavior (tag and behavior must agree)".into()
        });
    }
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
    if tags.contains(&BlockTag::BED) {
        if params.model_kind().is_none() {
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
    if tags.contains(&BlockTag::CLIMBABLE) {
        flags = flags.with(BlockFlags::CLIMBABLE);
    }
    if tags.contains(&BlockTag::SLIPPERY) {
        flags = flags.with(BlockFlags::SLIPPERY);
    }
    let front = match &r.front {
        None => None,
        Some(name) => {
            if !flags.is_directional_view() {
                return Err("a 'front' tile requires the 'directional_view' flag".into());
            }
            Some(tile(name)?)
        }
    };
    let panel_facing = match &r.panel_facing {
        None => {
            if sim.faces_by_row() {
                return Err(
                    "a ladder-shaped row must declare panel_facing (facing is block identity: \
                     one row per facing)"
                        .into(),
                );
            }
            None
        }
        Some(name) => {
            if !sim.faces_by_row() {
                return Err("panel_facing requires the 'ladder' shape".into());
            }
            Some(parse_facing(name)?)
        }
    };
    let facing_rows = match &r.facing_rows {
        None => None,
        Some(raw) => {
            if !sim.faces_by_row() {
                return Err("facing_rows requires the 'ladder' shape".into());
            }
            let resolve = |name: &String| {
                names
                    .blocks
                    .id(name)
                    .map(Block)
                    .ok_or_else(|| format!("unknown facing_rows block '{name}'"))
            };
            let rows: &'static [Block; 4] = Box::leak(Box::new([
                resolve(&raw.north)?,
                resolve(&raw.south)?,
                resolve(&raw.west)?,
                resolve(&raw.east)?,
            ]));
            Some(rows)
        }
    };
    let flipped_row = match &r.flipped {
        None => None,
        Some(name) => {
            if params.box_set().and_then(|b| b.run()).is_none() {
                return Err("'flipped' requires a '{\"run\": ...}' shape".into());
            }
            Some(
                names
                    .blocks
                    .id(name)
                    .map(Block)
                    .ok_or_else(|| format!("unknown flipped block '{name}'"))?,
            )
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
    let flow_tile = match &r.flow_tile {
        None => None,
        Some(name) => Some(tile(name)?),
    };
    if flow_tile.is_some() && !flags.fluid() {
        return Err("flow_tile is legal on a fluid row only".into());
    }
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
            Some(Box::leak(Box::new([**row])))
        }
    };
    let shape_kind = interner.intern(family, params, shape_key)?;
    let data = crate::registry::compile_data_map(
        names.blocks.name(block.id()).unwrap_or(""),
        &r.data,
        patches,
    )?;
    let rotate_y = crate::registry::engine_data::<String>(data, "petramond:rotate_y")?
        .map(|name| {
            names
                .blocks
                .id(&name)
                .map(Block)
                .ok_or_else(|| format!("Unknown rotate_y block: {name}"))
        })
        .transpose()?;
    let construction =
        match crate::registry::engine_data::<RawConstruction>(data, "petramond:construction")? {
            None => None,
            Some(RawConstruction::Item(name)) => Some(super::Construction::Item(
                names
                    .items
                    .id(&name)
                    .map(crate::item::ItemType)
                    .ok_or_else(|| format!("Unknown construction item: {name}"))?,
            )),
            Some(RawConstruction::Form(name)) => Some(super::Construction::Form(
                names
                    .blocks
                    .id(&name)
                    .map(Block)
                    .ok_or_else(|| format!("Unknown construction form: {name}"))?,
            )),
            Some(RawConstruction::Unsupported(reason)) => {
                Some(super::Construction::Unsupported(String::leak(reason)))
            }
        };
    let harvest_tier = crate::registry::engine_data::<RawHarvest>(data, "petramond:harvest")?
        .map_or(0, |h| h.tier);
    let carry: &'static [&'static str] =
        match crate::registry::engine_data::<Vec<String>>(data, "petramond:carry")? {
            None => &[],
            Some(keys) => {
                crate::registry::validate_namespaced_keys("petramond:carry", &keys)?;
                leak(
                    keys.into_iter()
                        .map(|k| -> &'static str { String::leak(k) })
                        .collect(),
                )
            }
        };
    if flags.fluid() != r.fluid.is_some() {
        return Err("a fluid flag and fluid properties must be declared together".into());
    }
    let fluid = r
        .fluid
        .map(|f| f.resolve(block, names).map(|f| &*Box::leak(Box::new(f))))
        .transpose()?;
    Ok(BlockDef {
        block,
        flags,
        contained_fluid,
        melts_to,
        fluid,
        tags: leak(tags),
        behavior,
        interaction,
        shape_kind,
        collision: leak(r.collision),
        emission: r.emission,
        emission_rgb: resolve_emission_rgb(r.emission, r.light_color)?,
        particle_emitter,
        tiles,
        uv_turns,
        front,
        side_overlay,
        covered_side,
        flow_tile,
        material: r.material,
        harvest_tier,
        hardness: r.hardness as f32,
        drop: DropSpec { drops: leak(drops) },
        next_stage,
        grows_into: leak(grows_into),
        panel_facing,
        animated_model,
        facing_rows,
        flipped_row,
        data,
        rotate_y,
        construction,
        carry,
        support: r.support,
        roots_on: leak(
            r.roots_on
                .iter()
                .map(|t| BlockTag::resolve(t))
                .collect::<Result<Vec<_>, String>>()?,
        ),
        roots_face: r.roots_face,
    })
}

fn resolve_growth_feature(key: &str) -> Result<&'static str, String> {
    Ok(String::leak(key.to_owned()))
}

pub fn validate_particle_emitter(e: &ParticleEmitter) -> Result<(), String> {
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
    finite("self_lit", e.self_lit)?;
    if !(0.0..=1.0).contains(&e.self_lit) {
        return Err("particle_emitter.self_lit must be in 0..=1".into());
    }
    finite("gravity", e.gravity)?;
    if e.gravity < 0.0 {
        return Err("particle_emitter.gravity must be >= 0 (it pulls down)".into());
    }
    if e.lands && e.gravity <= 0.0 && e.velocity[1] >= 0.0 {
        return Err(
            "particle_emitter.lands needs a downward motion: gravity or a negative velocity".into(),
        );
    }
    Ok(())
}

fn leak<T>(v: Vec<T>) -> &'static [T] {
    Box::leak(v.into_boxed_slice())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::ShapeFamily;

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
    fn bed_tagged_rows_must_be_sleepable_model_blocks() {
        let (base, _) =
            crate::assets::read_base_text("blocks.json").expect("assets/blocks.json must ship");
        let cube = r#"{ "blocks": [ { "block": "petramond:stone", "shape": "cube", "flags": ["solid", "opaque", "ao_occluder"], "tags": ["bed"], "behavior": "inert", "interaction": "sleep", "collision": [{"min": [0, 0, 0], "max": [1, 1, 1]}], "emission": 0, "tiles": ["stone", "stone", "stone"], "material": "stone", "data": {"petramond:harvest": {"tier": 1}}, "hardness": 1, "drops": [] } ] }"#;
        let err = parse_test_layers(&[&base, cube])
            .err()
            .expect("bed tag on a cube refused");
        assert!(err.contains("model shape"), "{err}");
        let unsleepable = r#"{ "blocks": [ { "block": "petramond:bed", "shape": {"model": "petramond:bed"}, "flags": ["solid", "directional_view"], "tags": ["bed"], "behavior": "inert", "interaction": "none", "collision": [], "emission": 0, "tiles": ["oak_planks", "oak_planks", "oak_planks"], "material": "wood", "hardness": 1, "drops": [] } ] }"#;
        let err = parse_test_layers(&[&base, unsleepable])
            .err()
            .expect("unsleepable bed tag refused");
        assert!(err.contains("interaction 'sleep'"), "{err}");
        let sleep_only = r#"{ "blocks": [ { "block": "petramond:bed", "shape": {"model": "petramond:bed"}, "flags": ["solid", "directional_view"], "tags": [], "behavior": "inert", "interaction": "sleep", "collision": [], "emission": 0, "tiles": ["oak_planks", "oak_planks", "oak_planks"], "material": "wood", "hardness": 1, "drops": [] } ] }"#;
        parse_test_layers(&[&base, sleep_only]).expect("sleep without the bed tag loads");
    }

    #[test]
    fn uv_rotation_resolves_to_slot_turns_and_rejects_the_rest() {
        let (base, _) =
            crate::assets::read_base_text("blocks.json").expect("assets/blocks.json must ship");
        let row = |uv: &str, shape: &str| {
            format!(
                r#"{{ "blocks": [ {{ "block": "petramond:stone", "shape": {shape}, "flags": ["solid", "opaque", "ao_occluder"], "tags": [], "behavior": "inert", "interaction": "none", "collision": [{{"min": [0, 0, 0], "max": [1, 1, 1]}}], "emission": 0, "tiles": ["stone", "stone", "stone"], "uv_rotation": {uv}, "material": "stone", "hardness": 1, "drops": [] }} ] }}"#
            )
        };
        let reg = parse_test_layers(&[&base, &row(r#"{"top": 90, "bottom": 270}"#, r#""cube""#)])
            .expect("a quarter-turn uv_rotation loads");
        assert_eq!(
            reg.defs[Block::Stone.id() as usize].uv_turns,
            [1, 3, 0],
            "top 90, bottom 270, side unturned"
        );

        let err = parse_test_layers(&[&base, &row(r#"{"top": 45}"#, r#""cube""#)])
            .err()
            .expect("a non-quarter turn refused");
        assert!(err.contains("must be 0, 90, 180 or 270"), "{err}");
        let err = parse_test_layers(&[&base, &row(r#"{"up": 90}"#, r#""cube""#)])
            .err()
            .expect("an unknown slot refused");
        assert!(err.contains("unknown uv_rotation slot"), "{err}");
        let boxes_row = r#"{"blocks": [{"block": "mymod:slab", "shape": {"boxes": [{"to": [16, 8, 16]}]}, "flags": ["solid"], "tags": [], "behavior": "inert", "interaction": "none", "collision": [], "emission": 0, "tiles": ["stone", "stone", "stone"], "uv_rotation": {"top": 90}, "material": "stone", "hardness": 1, "drops": []}]}"#;
        let err = parse_test_layers(&[&base, boxes_row])
            .err()
            .expect("uv_rotation on a box row refused");
        assert!(err.contains("cube/stair/slab"), "{err}");
    }

    #[test]
    fn fluids_that_quench_each_other_fail_the_load() {
        let (base, _) =
            crate::assets::read_base_text("blocks.json").expect("assets/blocks.json must ship");
        let table: serde_json::Value = serde_json::from_str(&base).unwrap();
        let mut rows: Vec<serde_json::Value> = table["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["fluid"].is_object())
            .take(2)
            .cloned()
            .collect();
        assert_eq!(rows.len(), 2, "the base table ships two fluid rows");
        let names: Vec<String> = rows
            .iter()
            .map(|r| r["block"].as_str().unwrap().to_owned())
            .collect();
        for (row, other) in rows.iter_mut().zip(names.iter().rev()) {
            row["fluid"]["quench"] = serde_json::json!({"by": other, "result": "petramond:stone"});
        }
        let layer = serde_json::json!({ "blocks": rows }).to_string();
        let err = parse_test_layers(&[&base, &layer])
            .err()
            .expect("a mutual quench pair is refused");
        assert!(err.contains("quench each other"), "{err}");
    }

    #[test]
    fn pack_layer_overrides_rows_by_block() {
        let (base, _) =
            crate::assets::read_base_text("blocks.json").expect("assets/blocks.json must ship");
        let layer = r#"{ "blocks": [ { "block": "petramond:stone", "shape": "cube", "flags": ["solid", "opaque", "ao_occluder"], "tags": ["terrain"], "behavior": "inert", "interaction": "none", "collision": [{"min": [0, 0, 0], "max": [1, 1, 1]}], "emission": 0, "tiles": ["stone", "stone", "stone"], "material": "stone", "data": {"petramond:harvest": {"tier": 1}}, "hardness": 99, "drops": [] } ] }"#;
        let base_reg = parse(&base).expect("base table loads");
        let reg = parse_test_layers(&[&base, layer]).expect("layered table loads");
        assert_eq!(
            reg.defs[Block::Stone.id() as usize].hardness,
            99.0,
            "the pack layer's stone row replaces the base row"
        );
        assert_eq!(reg.defs.len(), crate::block::ENGINE_BLOCK_NAMES.len());
        assert_eq!(
            reg.defs[Block::Dirt.id() as usize].hardness,
            base_reg.defs[Block::Dirt.id() as usize].hardness
        );
    }

    #[test]
    fn a_pack_patch_retunes_the_harvest_gate_of_a_row_it_does_not_own() {
        let (base, _) =
            crate::assets::read_base_text("blocks.json").expect("assets/blocks.json must ship");
        let base_reg = parse(&base).expect("base table loads");
        let shipped = base_reg.defs[Block::OakLog.id() as usize].harvest_tier;
        let wanted = if shipped == 0 { 3 } else { 0 };
        let patch = format!(
            r#"{{ "blocks": [ {{ "patch": "petramond:oak_log", "data": {{"petramond:harvest": {{"tier": {wanted}}}}} }} ] }}"#
        );
        let reg = parse_test_layers(&[&base, &patch]).expect("patched table loads");
        assert_eq!(
            reg.defs[Block::OakLog.id() as usize].harvest_tier,
            wanted,
            "the patch layer's harvest entry drives the compiled gate"
        );
        assert_eq!(reg.defs.len(), crate::block::ENGINE_BLOCK_NAMES.len());
        assert_eq!(
            reg.defs[Block::Stone.id() as usize].harvest_tier,
            base_reg.defs[Block::Stone.id() as usize].harvest_tier
        );
    }

    #[test]
    fn a_row_with_no_harvest_entry_is_hand_harvestable() {
        let (base, _) =
            crate::assets::read_base_text("blocks.json").expect("assets/blocks.json must ship");
        let layer = r#"{ "blocks": [ { "block": "mymod:chalk", "shape": "cube", "flags": ["solid", "opaque", "ao_occluder"], "tags": [], "behavior": "inert", "interaction": "none", "collision": [{"min": [0, 0, 0], "max": [1, 1, 1]}], "emission": 0, "tiles": ["stone", "stone", "stone"], "material": "stone", "hardness": 1, "drops": [] } ] }"#;
        let reg = parse_test_layers(&[&base, layer]).expect("unstated gate loads");
        let chalk = reg.defs.last().expect("the pack row registered");
        assert_eq!(chalk.harvest_tier, 0, "an absent entry means tier 0");
        let bad = r#"{ "blocks": [ { "block": "mymod:chalk", "shape": "cube", "flags": ["solid", "opaque", "ao_occluder"], "tags": [], "behavior": "inert", "interaction": "none", "collision": [{"min": [0, 0, 0], "max": [1, 1, 1]}], "emission": 0, "tiles": ["stone", "stone", "stone"], "material": "stone", "data": {"petramond:harvest": {"teir": 2}}, "hardness": 1, "drops": [] } ] }"#;
        let err = parse_test_layers(&[&base, bad])
            .err()
            .expect("a misspelled harvest key is refused");
        assert!(err.contains("petramond:harvest"), "{err}");
    }

    fn lamp_layer(flags: &str, emission: u8, light_color: &str) -> String {
        format!(
            r#"{{ "blocks": [ {{ "block": "mymod:lamp", "shape": "cube", "flags": [{flags}], "tags": [], "behavior": "inert", "interaction": "none", "collision": [{{"min": [0, 0, 0], "max": [1, 1, 1]}}], "emission": {emission}{light_color}, "tiles": ["stone", "stone", "stone"], "material": "stone", "data": {{"petramond:harvest": {{"tier": 1}}}}, "hardness": 2, "drops": [] }} ] }}"#
        )
    }

    fn lamp_emission_rgb(emission: u8, light_color: &str) -> Result<[u8; 3], String> {
        let flags = r#""solid", "opaque", "ao_occluder""#;
        let (base, _) =
            crate::assets::read_base_text("blocks.json").expect("assets/blocks.json must ship");
        let reg = parse_test_layers(&[&base, &lamp_layer(flags, emission, light_color)])?;
        Ok(reg.defs[crate::block::ENGINE_BLOCK_NAMES.len()].emission_rgb)
    }

    #[test]
    fn a_light_colour_rations_its_emission_and_never_exceeds_it() {
        let (base, _) =
            crate::assets::read_base_text("blocks.json").expect("assets/blocks.json must ship");
        let reg = parse(&base).expect("base table loads");
        for d in reg.defs {
            let peak = d.emission_rgb.iter().copied().max().unwrap_or(0);
            assert!(
                peak <= d.emission,
                "{:?} radiates {peak} on its brightest channel but declares emission {}",
                d.block,
                d.emission
            );
            assert_eq!(
                d.emission == 0,
                d.emission_rgb == [0; 3],
                "{:?}: scalar and per-channel disagree on whether it emits",
                d.block
            );
        }
        assert_eq!(lamp_emission_rgb(28, ""), Ok([28; 3]));
        assert_eq!(
            lamp_emission_rgb(28, r#", "light_color": [1.0, 1.0, 1.0]"#),
            Ok([28; 3])
        );
        assert_eq!(
            lamp_emission_rgb(28, r#", "light_color": [1.0, 0.5, 0.0]"#),
            Ok([28, 14, 0])
        );
    }

    #[test]
    fn a_meaningless_light_colour_fails_the_load() {
        let err = lamp_emission_rgb(28, r#", "light_color": [1.5, 1.0, 1.0]"#)
            .expect_err("out-of-range channel refused");
        assert!(err.contains("0.0..=1.0"), "{err}");
        let err = lamp_emission_rgb(0, r#", "light_color": [1.0, 0.5, 0.0]"#)
            .expect_err("colour on a non-emitter refused");
        assert!(err.contains("emission 0"), "{err}");
        let err = lamp_emission_rgb(1, r#", "light_color": [0.2, 0.2, 0.2]"#)
            .expect_err("colour that rounds every channel to zero refused");
        assert!(err.contains("zero on every channel"), "{err}");
    }

    #[test]
    fn an_emission_brighter_than_full_daylight_fails_the_load() {
        let over = crate::chunk::SKY_FULL + 1;
        for colour in ["", r#", "light_color": [1.0, 0.5, 0.2]"#] {
            let err = lamp_emission_rgb(over, colour).expect_err("over-bright emission refused");
            assert!(err.contains("exceeds the maximum light level"), "{err}");
        }
        assert!(lamp_emission_rgb(crate::chunk::SKY_FULL, "").is_ok());
    }

    #[test]
    fn namespaced_pack_row_registers_a_new_block() {
        let (base, _) =
            crate::assets::read_base_text("blocks.json").expect("assets/blocks.json must ship");
        let layer = r#"{ "blocks": [ { "block": "mymod:glowrock", "shape": "cube", "flags": ["solid", "opaque", "ao_occluder"], "tags": [], "behavior": "inert", "interaction": "none", "collision": [{"min": [0, 0, 0], "max": [1, 1, 1]}], "emission": 28, "tiles": ["stone", "stone", "stone"], "material": "stone", "data": {"petramond:harvest": {"tier": 1}}, "hardness": 2, "drops": [{"item": "petramond:cobblestone", "min": 1, "max": 1, "chance": 1.0}] } ] }"#;
        let reg = parse_test_layers(&[&base, layer]).expect("dynamic row loads");
        let engine = crate::block::ENGINE_BLOCK_NAMES.len();
        assert_eq!(
            reg.defs.len(),
            engine + 1,
            "one fresh id past the engine set"
        );
        let def = &reg.defs[engine];
        assert_eq!(def.block, Block(engine as u16));
        assert!(def.flags.is_solid() && def.flags.is_opaque());
        assert_eq!(def.behavior.key(), "inert");
        assert_eq!(def.emission, 28);
        assert_eq!(def.drop.drops.len(), 1);
        assert_eq!(def.drop.drops[0].item, ItemType::Cobblestone);
        assert_eq!(reg.defs[Block::Stone.id() as usize].block, Block::Stone);
    }

    #[test]
    fn a_row_declares_which_cell_holds_it_up() {
        let (base, _) =
            crate::assets::read_base_text("blocks.json").expect("assets/blocks.json must ship");
        let vine = |support: &str| {
            format!(
                r#"{{ "blocks": [ {{ "block": "mymod:vine", "shape": "cross", "flags": ["transparent"], "tags": ["fragile"], "behavior": "fragile", "interaction": "none", "collision": [], "emission": 0{support}, "tiles": ["poppy", "poppy", "poppy"], "material": "plant", "hardness": 0, "drops": [] }} ] }}"#
            )
        };
        let engine = crate::block::ENGINE_BLOCK_NAMES.len();
        let dir = |support: &str| {
            parse_test_layers(&[&base, &vine(support)]).map(|reg| reg.defs[engine].support)
        };
        assert_eq!(dir(""), Ok(definition::SupportDir::Below));
        assert_eq!(
            dir(r#", "support": "above""#),
            Ok(definition::SupportDir::Above)
        );
        assert!(dir(r#", "support": "sideways""#).is_err());
        let shipped = parse(&base).expect("base table loads");
        assert!(shipped.defs.iter().all(|d| d.support.is_default()));
    }

    #[test]
    fn roots_on_names_ground_tags_and_refuses_a_tag_nothing_carries() {
        let (base, _) =
            crate::assets::read_base_text("blocks.json").expect("assets/blocks.json must ship");
        let ground = r#"{ "block": "mymod:ash", "shape": "cube", "flags": ["solid", "opaque", "ao_occluder"], "tags": ["mymod:ashen"], "behavior": "inert", "interaction": "none", "collision": [{"min": [0, 0, 0], "max": [1, 1, 1]}], "emission": 0, "tiles": ["stone", "stone", "stone"], "material": "stone", "data": {"petramond:harvest": {"tier": 1}}, "hardness": 1, "drops": [] }"#;
        let plant = |roots_on: &str| {
            format!(
                r#"{{ "blocks": [ {ground}, {{ "block": "mymod:ashbloom", "shape": "cross", "flags": ["transparent"], "tags": ["fragile"], "roots_on": {roots_on}, "behavior": "fragile", "interaction": "none", "collision": [], "emission": 0, "tiles": ["poppy", "poppy", "poppy"], "material": "plant", "hardness": 0, "drops": [] }} ] }}"#
            )
        };
        let engine = crate::block::ENGINE_BLOCK_NAMES.len();
        let reg = parse_test_layers(&[&base, &plant(r#"["mymod:ashen", "soil"]"#)])
            .expect("a tag some row carries resolves");
        let bloom = &reg.defs[engine + 1];
        assert_eq!(bloom.roots_on.len(), 2);
        assert!(
            parse_test_layers(&[&base, &plant(r#"["mymod:ashn"]"#)]).is_err(),
            "a substrate tag no block carries must fail the load"
        );
    }

    #[test]
    fn custom_connection_shapes_load_resolve_and_validate() {
        use crate::block::ConnectionRule;
        let (base, _) =
            crate::assets::read_base_text("blocks.json").expect("assets/blocks.json must ship");
        let row = |name: &str, shape: &str| {
            format!(
                r#"{{ "blocks": [ {{ "block": "{name}", "shape": {shape}, "flags": [], "tags": [], "behavior": "inert", "interaction": "none", "collision": [], "emission": 0, "tiles": ["stone", "stone", "stone"], "material": "stone", "data": {{"petramond:harvest": {{"tier": 1}}}}, "hardness": 2, "drops": [] }} ] }}"#
            )
        };
        let engine = crate::block::ENGINE_BLOCK_NAMES.len();

        let wall = row(
            "mymod:stone_wall",
            r#"{"custom": {"family": "fence", "post_thickness": 8}}"#,
        );
        let reg = parse_test_layers(&[&base, &wall]).expect("custom fence wall loads");
        let def = &reg.defs[engine];
        let sk = &reg.shape_kinds[def.shape_kind.0 as usize];
        assert_eq!(sk.family, ShapeFamily::Fence);
        let c = sk.params.connection().expect("connection params");
        assert_eq!(c.post_lo, 4.0 / 16.0);
        assert_eq!(c.post_hi, 12.0 / 16.0);
        assert_eq!(c.rule, ConnectionRule::OpaqueOrSame);
        let post = crate::connect::boxes_for_mask(c.boxes, 0)[0];
        assert_eq!(post.min, [4.0 / 16.0, 0.0, 4.0 / 16.0]);

        let bar = row(
            "mymod:iron_bars",
            r#"{"custom": {"family": "pane", "post_thickness": 2, "connection_rule": "same_family_only"}}"#,
        );
        let reg = parse_test_layers(&[&base, &bar]).expect("custom pane bar loads");
        let sk = &reg.shape_kinds[reg.defs[engine].shape_kind.0 as usize];
        assert_eq!(sk.family, ShapeFamily::Pane);
        assert_eq!(
            sk.params.connection().unwrap().rule,
            ConnectionRule::SameOnly
        );

        for (shape, needle) in [
            (
                r#"{"custom": {"family": "bogus"}}"#,
                "unknown custom shape family",
            ),
            (
                r#"{"custom": {"family": "fence", "post_thickness": 0}}"#,
                "post_thickness",
            ),
            (
                r#"{"custom": {"family": "fence", "post_thickness": 10, "post_offset": 10}}"#,
                "exceeds 16",
            ),
            (
                r#"{"custom": {"family": "fence", "connection_rule": "nope"}}"#,
                "unknown connection_rule",
            ),
            (
                r#"{"custom": {"family": "fence", "item_form": "nope"}}"#,
                "unknown item_form",
            ),
            (
                r#"{"custom": {"family": "pane", "item_form": "segment"}}"#,
                "requires the 'fence' family",
            ),
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
        let layer = r#"{ "blocks": [ { "block": "mymod:spark", "shape": "cube", "flags": ["solid"], "tags": [], "behavior": "inert", "interaction": "none", "collision": [{"min": [0, 0, 0], "max": [1, 1, 1]}], "emission": 0, "particle_emitter": { "anchor": "block_top", "rate": [1.0, 2.0], "lifetime": [0.2, 0.4], "size": [0.02, 0.05], "spawn_box": [0.1, 0.0, 0.1], "velocity": [0.0, 0.2, 0.0], "velocity_jitter": [0.03, 0.02, 0.03], "color": [[1.0, 0.2, 0.0], [1.0, 1.0, 0.2]], "alpha": [0.2, 0.6], "self_lit": 1.0 }, "tiles": ["stone", "stone", "stone"], "material": "stone", "data": {"petramond:harvest": {"tier": 1}}, "hardness": 2, "drops": [] } ] }"#;
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
        let layer = r#"{ "blocks": [ { "block": "mymod:bad_spark", "shape": "cube", "flags": [], "tags": [], "behavior": "inert", "interaction": "none", "collision": [], "emission": 0, "particle_emitter": { "rate": 2.0, "lifetime": [0.5, 0.2], "size": [0.02, 0.05], "color": [[1.0, 0.2, 0.0], [1.0, 1.0, 0.2]], "alpha": [0.2, 0.6] }, "tiles": ["stone", "stone", "stone"], "material": "stone", "hardness": 1, "drops": [] } ] }"#;
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
                r#"{{ "blocks": [ {{ "block": "mymod:spark", "shape": "cube", "flags": [], "tags": [], "behavior": "inert", "interaction": "none", "collision": [], "emission": 0, "particle_emitter": {{ "rate": 2.0, "lifetime": [0.2, 0.5], "size": [0.02, 0.05], "alpha": [0.2, 0.6]{emitter} }}, "tiles": ["stone", "stone", "stone"], "material": "stone", "hardness": 1, "drops": [] }} ] }}"#
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

    #[test]
    fn sapling_rows_validate_tag_behavior_and_stage_chain() {
        let (base, _) =
            crate::assets::read_base_text("blocks.json").expect("assets/blocks.json must ship");
        let row = |name: &str, tags: &str, behavior: &str, growth: &str| {
            format!(
                r#"{{ "blocks": [ {{ "block": "{name}", {growth} "shape": "cross", "flags": ["transparent"], "tags": [{tags}], "behavior": "{behavior}", "interaction": "none", "collision": [], "emission": 0, "tiles": ["oak_sapling", "oak_sapling", "oak_sapling"], "material": "plant", "hardness": 0, "drops": [] }} ] }}"#
            )
        };
        let sapling_tags = r#""fragile", "roots_in_soil", "sapling""#;

        let good = row(
            "mymod:sap",
            sapling_tags,
            "sapling",
            r#""grows_into": [{"feature": "petramond:oak_big", "weight": 1}, {"feature": "petramond:oak_small"}],"#,
        );
        parse_test_layers(&[&base, &good]).expect("a valid final-stage sapling row loads");
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

    #[test]
    fn run_rows_validate_the_flipped_sibling_and_refuse_a_facing() {
        let (base, _) =
            crate::assets::read_base_text("blocks.json").expect("assets/blocks.json must ship");
        let run = |root: &str| {
            format!(
                r#"{{"run":{{"root":"{root}","forms":{{"tip":[{{"from":[6,0,6],"to":[10,16,10]}}],"frustum":[{{"from":[4,0,4],"to":[12,16,12]}}],"middle":[{{}}],"base":[{{}}]}}}}}}"#
            )
        };
        let row = |name: &str, shape: &str, extra: &str| {
            format!(
                r#"{{ "block": "{name}", "shape": {shape}, {extra} "flags": ["solid"], "tags": [], "behavior": "inert", "interaction": "none", "collision": [], "emission": 0, "tiles": ["stone", "stone", "stone"], "material": "stone", "hardness": 1.0, "drops": [] }}"#
            )
        };
        let layer = |rows: &[String]| format!(r#"{{ "blocks": [ {} ] }}"#, rows.join(","));
        let pair = layer(&[
            row("mymod:spike", &run("down"), r#""flipped": "mymod:icicle","#),
            row("mymod:icicle", &run("up"), r#""flipped": "mymod:spike","#),
        ]);
        parse_test_layers(&[&base, &pair]).expect("a mirrored pair loads");
        let lone = layer(&[row("mymod:spike", &run("down"), "")]);
        parse_test_layers(&[&base, &lone]).expect("a run with no sibling loads");
        for (layer, why, needle) in [
            (
                layer(&[
                    row("mymod:spike", &run("down"), r#""flipped": "mymod:icicle","#),
                    row("mymod:icicle", &run("down"), r#""flipped": "mymod:spike","#),
                ]),
                "a sibling rooted the same way",
                "not a run rooted 'up'",
            ),
            (
                layer(&[
                    row("mymod:spike", &run("down"), r#""flipped": "mymod:icicle","#),
                    row("mymod:icicle", &run("up"), ""),
                ]),
                "a sibling that does not name the row back",
                "name it back",
            ),
            (
                layer(&[row(
                    "mymod:spike",
                    r#""cube""#,
                    r#""flipped": "petramond:stone","#,
                )]),
                "flipped on a non-run row",
                "requires a",
            ),
            (
                layer(&[row(
                    "mymod:spike",
                    &run("down"),
                    r#""flags": ["solid", "directional_view"], "front": "stone","#,
                )
                .replacen(r#""flags": ["solid"], "#, "", 1)]),
                "a run with a stored facing",
                "directional_view",
            ),
        ] {
            let err = parse_test_layers(&[&base, &layer])
                .err()
                .unwrap_or_else(|| panic!("{why} must fail the load"));
            assert!(err.contains(needle), "{why}: {err}");
        }
    }

    #[test]
    fn landing_emitters_need_a_downward_motion() {
        let row = |extra: &str| {
            serde_json::from_str::<ParticleEmitter>(&format!(
                r#"{{"rate": 1, "lifetime": [1, 1], "size": [0.1, 0.1], "alpha": [1, 1], "color": [[0,0,1],[0,0,1]]{extra}}}"#
            ))
            .expect("row parses")
        };
        assert!(validate_particle_emitter(&row("")).is_ok());
        assert!(validate_particle_emitter(&row(r#", "gravity": 12, "lands": true"#)).is_ok());
        assert!(
            validate_particle_emitter(&row(r#", "velocity": [0, -1, 0], "lands": true"#)).is_ok()
        );
        assert!(validate_particle_emitter(&row(r#", "lands": true"#)).is_err());
        assert!(validate_particle_emitter(&row(r#", "gravity": -1"#)).is_err());
    }

    #[test]
    fn wall_panel_rows_validate_facing_identity_and_sibling_map() {
        let (base, _) =
            crate::assets::read_base_text("blocks.json").expect("assets/blocks.json must ship");
        let row = |name: &str, shape: &str, extra: &str| {
            format!(
                r#"{{ "blocks": [ {{ "block": "{name}", "shape": "{shape}", {extra} "flags": ["transparent"], "tags": ["fragile", "climbable"], "behavior": "fragile", "interaction": "none", "collision": [], "emission": 0, "tiles": ["ladder", "ladder", "ladder"], "material": "wood", "hardness": 0.4, "drops": [] }} ] }}"#
            )
        };

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
        let layer = r#"{ "blocks": [ { "block": "glowrock", "shape": "cube", "flags": [], "tags": [], "behavior": "inert", "interaction": "none", "collision": [], "emission": 0, "tiles": ["stone", "stone", "stone"], "material": "none", "hardness": 1, "drops": [] } ] }"#;
        let err = parse_test_layers(&[&base, layer])
            .err()
            .expect("bare additions are refused");
        assert!(
            err.contains("glowrock") && err.contains("namespace"),
            "{err}"
        );
    }

    #[test]
    fn open_gui_interaction_parses_namespaced_and_rejects_bare() {
        let (base, _) =
            crate::assets::read_base_text("blocks.json").expect("assets/blocks.json must ship");
        let layer = r#"{ "blocks": [ { "block": "guimod:opener", "shape": "cube", "flags": ["solid", "opaque", "ao_occluder"], "tags": [], "behavior": "inert", "interaction": {"open_gui": "guimod:panel"}, "collision": [{"min": [0, 0, 0], "max": [1, 1, 1]}], "emission": 0, "tiles": ["stone", "stone", "stone"], "material": "stone", "data": {"petramond:harvest": {"tier": 1}}, "hardness": 2, "drops": [] } ] }"#;
        let reg = parse_test_layers(&[&base, layer]).expect("open_gui row loads");
        let def = &reg.defs[crate::block::ENGINE_BLOCK_NAMES.len()];
        let BlockInteraction::OpenGui(kind) = def.interaction else {
            panic!("expected OpenGui, got {:?}", def.interaction);
        };
        assert_eq!(crate::gui_state::kind_key(kind), Some("guimod:panel"));

        let bare = layer.replace("guimod:panel", "panel");
        let err = parse_test_layers(&[&base, &bare]).err().unwrap();
        assert!(err.contains("namespaced"), "{err}");

        let unknown = layer.replace(r#"{"open_gui": "guimod:panel"}"#, r#""bogus_action""#);
        let err = parse_test_layers(&[&base, &unknown]).err().unwrap();
        assert!(err.contains("unknown interaction"), "{err}");
    }

    #[test]
    fn loader_rejects_incomplete_or_unknown_rows() {
        let partial = r#"{ "blocks": [ { "block": "petramond:air", "shape": "cube", "flags": [], "tags": [], "behavior": "inert", "interaction": "none", "collision": [], "emission": 0, "tiles": ["dirt", "dirt", "dirt"], "material": "none", "hardness": -1, "drops": [] } ] }"#;
        assert!(parse(partial).err().unwrap().contains("missing row"));
        let (base, _) =
            crate::assets::read_base_text("blocks.json").expect("assets/blocks.json must ship");
        let bad_behavior = r#"{ "blocks": [ { "block": "petramond:air", "shape": "cube", "flags": [], "tags": [], "behavior": "bogus", "interaction": "none", "collision": [], "emission": 0, "tiles": ["dirt", "dirt", "dirt"], "material": "none", "hardness": -1, "drops": [] } ] }"#;
        assert!(parse_test_layers(&[&base, bad_behavior])
            .err()
            .unwrap()
            .contains("unknown behavior"));
        let bad_tile = bad_behavior.replace("\"bogus\"", "\"inert\"").replace(
            "\"dirt\", \"dirt\", \"dirt\"",
            "\"dirt\", \"dirt\", \"bogus_tile\"",
        );
        assert!(parse_test_layers(&[&base, &bad_tile])
            .err()
            .unwrap()
            .contains("unknown tile"));
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
