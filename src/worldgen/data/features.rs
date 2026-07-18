//! Configured tree features — a layered catalog (`assets/features.json`).
//!
//! Each tree is a data row: a SHAPE (which `Feature` implementation) plus its
//! materials and geometry params. Engine features own the low ids in the
//! frozen const order below; a mod pack ADDS a feature with a namespaced
//! (`mod_id:name`) key or OVERRIDES an engine row to retune it (see
//! [`crate::registry`]). Biome modules decide which configured feature to
//! place (`worldgen::biome::trees`), so a pack-added feature only generates
//! when something references it.
//!
//! What stays code: the `Feature`/placer implementations themselves, the
//! trunk-placer strategies (zero-sized, keyed by name here), and the
//! worldgen RNG-driven variant pickers (`biome::trees`; a SAPLING's tree
//! choices are block-row data — `grows_into`, resolved through [`by_name`]).
//! A row's
//! params affect worldgen geometry, so edits to `features.json` change world
//! bytes — determinism only demands same-input ⇒ same-output.

use std::sync::LazyLock;

use serde::Deserialize;

use crate::block::Block;
use crate::worldgen::feature::placers::foliage::{
    ConiferFoliage, DroopyFoliage, FlatSparseFoliage, FoliagePlacer,
};
use crate::worldgen::feature::placers::trunk::{LeaningTrunk, StraightTrunk, TrunkPlacer};
use crate::worldgen::feature::tree::{
    BlockyOakFeature, CanopyTreeFeature, RedwoodFeature, TreeFeature,
};
use crate::worldgen::feature::{ConfiguredFeature, Feature};

/// Engine feature names in frozen id order (the completeness oracle
/// `features.json` is validated against).
const ENGINE_FEATURE_NAMES: &[&str] = &[
    "petramond:oak_young",
    "petramond:oak_small",
    "petramond:oak_swamp",
    "petramond:oak_big",
    "petramond:redwood",
    "petramond:spruce",
    "petramond:birch",
    "petramond:jungle",
    "petramond:acacia",
];

// Shared trunk placers (zero-sized strategies the JSON names; height is
// per-tree config).
static STRAIGHT: StraightTrunk = StraightTrunk;
static LEANING: LeaningTrunk = LeaningTrunk;

/// One row of the loaded feature table.
pub struct FeatureDef {
    /// The row's registry name (`"petramond:oak_big"`, `"mod_id:palm"`).
    #[allow(dead_code)]
    pub name: &'static str,
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
}

/// Foliage placers carry their shape params as fields, so a row states the
/// family and its numbers.
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum RawFoliage {
    Droopy(DroopyFoliage),
    Conifer(ConiferFoliage),
    FlatSparse(FlatSparseFoliage),
}

impl RawShape {
    /// Build the `Feature` this row configures. Rows load once per process,
    /// so leaking the built feature is the static lifetime, not a leak.
    fn resolve(self) -> &'static dyn Feature {
        match self {
            RawShape::BlockyOak(f) => Box::leak(Box::new(f)),
            RawShape::Canopy(f) => Box::leak(Box::new(f)),
            RawShape::Redwood(f) => Box::leak(Box::new(f)),
            RawShape::Tree(t) => {
                let trunk: &'static dyn TrunkPlacer = match t.trunk {
                    RawTrunk::Straight => &STRAIGHT,
                    RawTrunk::Leaning => &LEANING,
                };
                let foliage: &'static dyn FoliagePlacer = match t.foliage {
                    RawFoliage::Droopy(f) => Box::leak(Box::new(f)),
                    RawFoliage::Conifer(f) => Box::leak(Box::new(f)),
                    RawFoliage::FlatSparse(f) => Box::leak(Box::new(f)),
                };
                Box::leak(Box::new(TreeFeature {
                    trunk,
                    foliage,
                    log: t.log,
                    leaf: t.leaf,
                    height: t.height,
                }))
            }
        }
    }
}

#[derive(Deserialize)]
struct RawFile {
    features: Vec<RawFeatureDef>,
}

fn catalog() -> &'static crate::registry::Catalog<FeatureDef> {
    static TABLE: LazyLock<crate::registry::Catalog<FeatureDef>> = LazyLock::new(|| {
        crate::registry::read_catalog("features.json", "worldgen feature", parse_layers)
    });
    &TABLE
}

fn parse_layers(texts: &[&str]) -> Result<crate::registry::Catalog<FeatureDef>, String> {
    crate::registry::load_catalog(
        texts,
        |text| serde_json::from_str::<RawFile>(text).map(|f| f.features),
        |r| &r.feature,
        ENGINE_FEATURE_NAMES,
        "worldgen feature",
        |r, id, names| {
            Ok(FeatureDef {
                name: names.name(id).expect("id resolved from this table"),
                configured: ConfiguredFeature {
                    feature: r.shape.resolve(),
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

/// The engine feature at its frozen id (`ENGINE_FEATURE_NAMES` order).
fn engine(id: usize) -> &'static ConfiguredFeature {
    &catalog().rows()[id].configured
}

pub fn oak_young() -> &'static ConfiguredFeature {
    engine(0)
}
pub fn oak_small() -> &'static ConfiguredFeature {
    engine(1)
}
pub fn oak_swamp() -> &'static ConfiguredFeature {
    engine(2)
}
pub fn oak_big() -> &'static ConfiguredFeature {
    engine(3)
}
pub fn redwood() -> &'static ConfiguredFeature {
    engine(4)
}
pub fn spruce() -> &'static ConfiguredFeature {
    engine(5)
}
// (birch/jungle have no worldgen picker or code accessor: worldgen never
// places them and saplings reach them through `by_name` via `grows_into`.)
pub fn acacia() -> &'static ConfiguredFeature {
    engine(8)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped `features.json` resolves every engine row against the real
    /// block registry, and pack rows register after the engine range.
    #[test]
    fn engine_rows_hold_frozen_ids_and_pack_rows_register_after() {
        let base = std::fs::read_to_string(
            crate::assets::candidate_paths("features.json")
                .into_iter()
                .find(|p| p.exists())
                .expect("shipped features.json"),
        )
        .unwrap();
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
        for (id, name) in ENGINE_FEATURE_NAMES.iter().enumerate() {
            assert_eq!(table.rows()[id].name, *name, "engine ids never move");
        }
        assert_eq!(
            table.id("mymod:palm"),
            Some(ENGINE_FEATURE_NAMES.len() as u8)
        );
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
        let unknown_block = r#"{"features": [{"feature": "petramond:redwood", "shape": {
            "redwood": {"log": "petramond:not_a_block", "leaf": "petramond:redwood_leaves",
            "height": [38, 52]}}}]}"#;
        assert!(parse_layers(&[unknown_block]).is_err(), "unknown block name");
    }
}
