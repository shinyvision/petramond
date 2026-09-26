//! Configured tree features — a layered catalog (`assets/features.json`).
//!
//! Each tree is a data row: a SHAPE (which `Feature` implementation) plus its
//! materials and geometry params. Engine features own the low ids in the
//! frozen const order below; a mod pack ADDS a feature with a namespaced
//! (`mod_id:name`) key or OVERRIDES an engine row to retune it (see
//! [`petramond_world::registry`]). Which feature a biome places is row data too
//! (the biome rows' `trees` species tables; a SAPLING's choices are block-row
//! `grows_into` data), both resolved through [`by_name`], so a pack-added
//! feature generates wherever a row names it.
//!
//! What stays code: the `Feature`/placer implementations themselves and the
//! trunk-placer strategies (zero-sized, keyed by name here). A row's params
//! affect worldgen geometry, so edits to `features.json` change world bytes —
//! determinism only demands same-input ⇒ same-output.

use petramond_world::content::{ContentRegistry, Slot};
use petramond_world::registry::Catalog;

use serde::Deserialize;

use crate::feature::placers::foliage::{
    ConiferFoliage, DroopyFoliage, FlatSparseFoliage, FoliagePlacer, LayeredFoliage,
};
use crate::feature::placers::trunk::{LeaningTrunk, StraightTrunk, TrunkPlacer, WhorledTrunk};
use crate::feature::tree::{BlockyOakFeature, CanopyTreeFeature, RedwoodFeature, TreeFeature};
use crate::feature::{ConfiguredFeature, Feature};
use petramond_world::block::Block;

/// Declares the engine features ONCE: their names in frozen id order (the
/// completeness oracle `features.json` is validated against) and a typed
/// accessor per feature that resolves by that same name, so an accessor can
/// neither drift from its row nor be missing for one.
macro_rules! engine_features {
    ($($accessor:ident => $name:literal),+ $(,)?) => {
        const ENGINE_FEATURE_NAMES: &[&str] = &[$($name),+];

        $(
            #[doc = concat!("The `", $name, "` feature row.")]
            pub fn $accessor() -> &'static ConfiguredFeature {
                engine($name)
            }
        )+
    };
}

engine_features! {
    oak_young => "petramond:oak_young",
    oak_small => "petramond:oak_small",
    oak_swamp => "petramond:oak_swamp",
    oak_big => "petramond:oak_big",
    redwood => "petramond:redwood",
    spruce => "petramond:spruce",
    birch => "petramond:birch",
    jungle => "petramond:jungle",
    acacia => "petramond:acacia",
}

// Shared trunk placers (zero-sized strategies the JSON names; height is
// per-tree config).
static STRAIGHT: StraightTrunk = StraightTrunk;
static LEANING: LeaningTrunk = LeaningTrunk;

/// One row of the loaded feature table.
pub struct FeatureDef {
    pub configured: ConfiguredFeature,
}

/// One feature row as written in `features.json`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFeatureDef {
    feature: String,
    shape: RawShape,
}

/// A row's `shape`: which `Feature` implementation builds it, plus that
/// shape's own required params (the target structs are closed — a missing or
/// stray field is a serde error). Oaks ride `blocky_oak` — the tuned concept
/// silhouette (wandering flared trunk, surface roots, levelled
/// turning/splitting branches, eroded cuboid leaf clumps). Other broadleaf
/// species (birch, jungle) use `canopy`, the rounded skeleton-and-clumps
/// silhouette. `tree` is the generic trunk + foliage composition for simple
/// trees.
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum RawShape {
    BlockyOak(BlockyOakFeature),
    Canopy(CanopyTreeFeature),
    Redwood(RedwoodFeature),
    Tree(RawTree),
}

/// The generic composition: `TreeFeature` holds placer trait objects, so its
/// row form names the trunk strategy and states the foliage family + params.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTree {
    trunk: RawTrunk,
    foliage: RawFoliage,
    log: Block,
    leaf: Block,
    height: (i32, i32),
}

/// Trunk strategies are genuinely code (zero-sized walk algorithms), so the
/// JSON references them by name.
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum RawTrunk {
    Straight,
    Leaning,
    Whorled(WhorledTrunk),
}

/// Foliage placers carry their shape params as fields, so a row states the
/// family and its numbers.
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum RawFoliage {
    Droopy(DroopyFoliage),
    Conifer(ConiferFoliage),
    FlatSparse(FlatSparseFoliage),
    Layered(LayeredFoliage),
}

impl RawShape {
    /// Build the `Feature` this row configures. Rows load once per process,
    /// so leaking the built feature is the static lifetime, not a leak.
    fn resolve(self) -> Result<&'static dyn Feature, String> {
        Ok(match self {
            RawShape::BlockyOak(f) => {
                f.validate()?;
                Box::leak(Box::new(f))
            }
            RawShape::Canopy(f) => {
                f.validate()?;
                Box::leak(Box::new(f))
            }
            RawShape::Redwood(f) => {
                f.validate()?;
                Box::leak(Box::new(f))
            }
            RawShape::Tree(t) => {
                crate::data::bounds::ascending("height", t.height, 5..=56)?;
                let trunk: &'static dyn TrunkPlacer = match t.trunk {
                    RawTrunk::Straight => &STRAIGHT,
                    RawTrunk::Leaning => &LEANING,
                    RawTrunk::Whorled(trunk) => {
                        trunk.validate(t.height)?;
                        Box::leak(Box::new(trunk))
                    }
                };
                let foliage: &'static dyn FoliagePlacer = match t.foliage {
                    RawFoliage::Droopy(f) => Box::leak(Box::new(f)),
                    RawFoliage::Conifer(f) => Box::leak(Box::new(f)),
                    RawFoliage::FlatSparse(f) => Box::leak(Box::new(f)),
                    RawFoliage::Layered(f) => {
                        f.validate()?;
                        Box::leak(Box::new(f))
                    }
                };
                // The canopy-open oracle reads column surfaces under every
                // leaf cell, and those reads must stay inside the candidate
                // window every replaying chunk can serve (the same fence the
                // oak anchoring gate documents on its root reach).
                let reach = foliage.horizontal_reach() + trunk.max_lean();
                if reach > crate::biome::trees::MAX_TREE_SPACING_RADIUS {
                    return Err(format!(
                        "foliage reach + trunk lean: {reach} blocks exceed the {} block candidate window",
                        crate::biome::trees::MAX_TREE_SPACING_RADIUS
                    ));
                }
                Box::leak(Box::new(TreeFeature {
                    trunk,
                    foliage,
                    log: t.log,
                    leaf: t.leaf,
                    height: t.height,
                }))
            }
        })
    }
}

#[derive(Deserialize)]
struct RawFile {
    features: Vec<RawFeatureDef>,
}

/// The feature catalog stage (see [`super::content_stages`]).
pub(crate) static CATALOG: Slot<Catalog<FeatureDef>> = Slot::new(
    "features.json",
    &[petramond_world::content::stage::BLOCKS],
    load,
);

fn load(reg: &ContentRegistry) -> Result<Catalog<FeatureDef>, String> {
    let table = petramond_world::registry::read_catalog(
        reg.packs(),
        "features.json",
        "worldgen feature",
        parse_layers,
    )?;
    // Cross-check every block row's `grows_into` key against this table. The
    // block layer sits BELOW worldgen and interns keys unchecked; a final
    // sapling stage naming a missing tree must still fail the load, and this
    // is the first layer that owns the feature names.
    let mut unknown = Vec::new();
    for block in petramond_world::block::Block::all() {
        for (key, _) in block.grows_into() {
            if table.id(key).is_none() {
                unknown.push(format!(
                    "blocks.json: '{}' grows_into names unknown worldgen feature '{key}'",
                    reg.names().blocks.name(block.id()).unwrap_or("?")
                ));
            }
        }
    }
    if !unknown.is_empty() {
        return Err(unknown.join("\n"));
    }
    Ok(table)
}

fn catalog() -> &'static Catalog<FeatureDef> {
    CATALOG.current()
}

fn parse_layers(texts: &[&str]) -> Result<petramond_world::registry::Catalog<FeatureDef>, String> {
    petramond_world::registry::load_catalog(
        texts,
        |text| serde_json::from_str::<RawFile>(text).map(|f| f.features),
        |r| &r.feature,
        ENGINE_FEATURE_NAMES,
        "worldgen feature",
        |r, id, names| {
            let name = names.name(id).expect("id resolved from this table");
            Ok(FeatureDef {
                configured: ConfiguredFeature {
                    feature: r
                        .shape
                        .resolve()
                        .map_err(|e| format!("worldgen feature '{name}': {e}"))?,
                },
            })
        },
    )
}

/// The configured feature registered under `name` (engine `petramond:*` and
/// pack `mod_id:name` keys alike), or `None` when no such row is loaded.
pub fn by_name(name: &str) -> Option<&'static ConfiguredFeature> {
    let c = catalog();
    c.id(name).map(|id| &c.rows()[id as usize].configured)
}

/// An engine feature by its key. Every engine name is a completeness
/// requirement of the catalog load, so the row always exists.
fn engine(name: &str) -> &'static ConfiguredFeature {
    by_name(name).unwrap_or_else(|| panic!("engine feature {name} is not loaded"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each generated accessor returns the row registered under its own
    /// name — including features no engine code places directly.
    #[test]
    fn accessors_resolve_their_own_rows() {
        let accessors: [(&str, fn() -> &'static ConfiguredFeature); 9] = [
            ("petramond:oak_young", oak_young),
            ("petramond:oak_small", oak_small),
            ("petramond:oak_swamp", oak_swamp),
            ("petramond:oak_big", oak_big),
            ("petramond:redwood", redwood),
            ("petramond:spruce", spruce),
            ("petramond:birch", birch),
            ("petramond:jungle", jungle),
            ("petramond:acacia", acacia),
        ];
        assert_eq!(accessors.len(), ENGINE_FEATURE_NAMES.len());
        for (name, accessor) in accessors {
            let row = by_name(name).expect("engine row loaded");
            assert!(std::ptr::eq(accessor(), row), "{name} accessor");
        }
    }

    /// The shipped `features.json` resolves every engine row against the real
    /// block registry, and pack rows register after the engine range.
    ///
    /// Reads the BASE layer only: "the shipped catalog is valid on its own"
    /// must not change meaning because a pack happens to layer this file.
    #[test]
    fn engine_rows_hold_frozen_ids_and_pack_rows_register_after() {
        let base = petramond_world::assets::read_base_text("features.json")
            .expect("shipped features.json")
            .0;
        let pack = r#"{"features": [
            {"feature": "petramond:spruce", "shape": {"tree": {
                "trunk": "straight",
                "foliage": {"conifer": {"radius": 3, "skirt_ragged": 0.5}},
                "log": "petramond:spruce_log", "leaf": "petramond:spruce_leaves",
                "height": [8, 12]}}},
            {"feature": "mymod:palm", "shape": {"tree": {
                "trunk": "leaning",
                "foliage": {"droopy": {"radius": 3, "ragged": 0.2, "drip_skip": 0.5}},
                "log": "petramond:jungle_log", "leaf": "petramond:jungle_leaves",
                "height": [6, 9]}}}
        ]}"#;
        let table = parse_layers(&[&base, pack]).expect("loads");
        assert_eq!(
            table.rows().len(),
            ENGINE_FEATURE_NAMES.len() + 1,
            "the engine override adds no id; the pack addition does"
        );
        for name in ENGINE_FEATURE_NAMES {
            assert!(
                table.id(name).is_some(),
                "engine row {name} still registered"
            );
        }
        assert!(table.id("mymod:palm").is_some(), "the pack row registered");
    }

    /// A shape's params are required and closed — a missing field or a stray
    /// one is a load error, not a silent default.
    #[test]
    fn shape_params_are_validated() {
        let missing = r#"{"features": [{"feature": "petramond:redwood", "shape": {
            "redwood": {"log": "petramond:redwood_log", "leaf": "petramond:redwood_leaves"}}}]}"#;
        assert!(parse_layers(&[missing]).is_err(), "missing height");
        let stray = r#"{"features": [{"feature": "petramond:redwood", "shape": {
            "redwood": {"log": "petramond:redwood_log", "leaf": "petramond:redwood_leaves",
            "height": [38, 52], "sparkle": 1}}}]}"#;
        assert!(parse_layers(&[stray]).is_err(), "unknown shape param");
        let towering = r#"{"features": [{"feature": "petramond:redwood", "shape": {
            "redwood": {"log": "petramond:redwood_log", "leaf": "petramond:redwood_leaves",
            "height": [50, 70]}}}]}"#;
        assert!(
            parse_layers(&[towering]).is_err(),
            "a crown past the tree reach"
        );
        let unknown_block = r#"{"features": [{"feature": "petramond:redwood", "shape": {
            "redwood": {"log": "petramond:not_a_block", "leaf": "petramond:redwood_leaves",
            "height": [38, 52]}}}]}"#;
        assert!(
            parse_layers(&[unknown_block]).is_err(),
            "unknown block name"
        );
    }
}

#[cfg(test)]
mod growth_target_tests {
    /// Every shipped block row's `grows_into` key resolves in the loaded
    /// feature catalog — the cross-layer invariant behind the catalog-load
    /// panic (the block layer interns keys unchecked; this layer owns the
    /// names). Forcing the catalog also exercises the panic path on drift.
    #[test]
    fn every_block_growth_target_resolves() {
        for block in petramond_world::block::Block::all() {
            for (key, _) in block.grows_into() {
                assert!(
                    super::by_name(key).is_some(),
                    "block row grows_into '{key}' missing from features.json"
                );
            }
        }
    }
}

#[cfg(test)]
mod posture_tests;
