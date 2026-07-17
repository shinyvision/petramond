use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

use crate::facing::Facing;

// ---------------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------------

/// A bbmodel kind — the registry key, one per authored model (an opaque runtime
/// id indexing the loaded def table + [`MODELS`]/[`INSTANCES`]). Engine kinds own
/// the low ids in the frozen const order below; mod packs register additional
/// kinds through namespaced `models.json` rows (see [`crate::registry`]) and
/// reference them from a block row's `shape` field. A [`RenderShape::Model`]
/// block names its kind; an ITEM-ONLY model item (no block, e.g. the bucket)
/// names its kind via `ItemType::render_kind` instead, so its
/// placement/collision machinery simply never runs.
///
/// Serde carries a kind as its registry KEY string (`furniture_workbench`).
///
/// [`RenderShape::Model`]: crate::block::RenderShape::Model
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct BlockModelKind(pub u8);

/// Engine model-kind consts, named like the enum variants they replaced.
#[allow(non_upper_case_globals)]
impl BlockModelKind {
    pub const FurnitureWorkbench: BlockModelKind = BlockModelKind(0);
    pub const Bucket: BlockModelKind = BlockModelKind(1);
    pub const WaterBucket: BlockModelKind = BlockModelKind(2);
    pub const BedFrame: BlockModelKind = BlockModelKind(3);
    pub const Bed: BlockModelKind = BlockModelKind(4);
}

/// Engine model keys in frozen id order — the completeness oracle
/// `models.json` is validated against.
const ENGINE_MODEL_KEYS: &[&str] = &[
    "petramond:furniture_workbench",
    "petramond:bucket",
    "petramond:water_bucket",
    "petramond:bed_frame",
    "petramond:bed",
];

impl std::fmt::Debug for BlockModelKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match ENGINE_MODEL_KEYS.get(self.0 as usize) {
            Some(key) => write!(f, "BlockModelKind({key})"),
            None => write!(f, "BlockModelKind(#{})", self.0),
        }
    }
}

impl Serialize for BlockModelKind {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(def(*self).key)
    }
}

impl<'de> Deserialize<'de> for BlockModelKind {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let key = std::borrow::Cow::<str>::deserialize(d)?;
        defs()
            .iter()
            .position(|m| m.key == key)
            .map(|i| BlockModelKind(i as u8))
            .ok_or_else(|| serde::de::Error::custom(format!("unknown block model '{key}'")))
    }
}

/// Every registered kind in id order (engine + pack-registered).
pub fn all() -> &'static [BlockModelKind] {
    static ALL: LazyLock<Vec<BlockModelKind>> = LazyLock::new(|| {
        (0..defs().len())
            .map(|id| BlockModelKind(id as u8))
            .collect()
    });
    &ALL
}

/// How a bbmodel block's player collision is derived. Resolved PER CELL: a multi-block
/// intersects the chosen shape with each occupied cell.
#[derive(Copy, Clone)]
pub enum CollisionSpec {
    /// Auto: the model's footprint bounds, split per cell (the default).
    FromModel,
}

/// How a placed model orients its authored X axis relative to the placing player
/// (multi-cell models and `DIRECTIONAL_VIEW` blocks orient on placement — see
/// `game::placement`).
#[derive(Copy, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlacementOrientation {
    /// Authored X spans LEFT-TO-RIGHT across the player's view; the authored front
    /// (−Z) faces the player. Furniture you stand in front of (the workbench).
    LeftToRight,
    /// Quarter-turned from [`LeftToRight`](Self::LeftToRight): authored X runs
    /// FRONT-TO-BACK along the player's view, with the clicked cell at the near,
    /// authored-max-X end and authored −X growing away — a bed placed foot-first,
    /// headboard at the far end.
    FrontToBack,
}

impl PlacementOrientation {
    /// The stored facing for a model placed by a player whose facing (front toward
    /// the player, from `facing_from_forward`) is `player_facing`.
    pub fn apply(self, player_facing: Facing) -> Facing {
        match self {
            PlacementOrientation::LeftToRight => player_facing,
            // The quarter turn that sends the authored −X (far) end away from the
            // player: N→W, W→S, S→E, E→N.
            PlacementOrientation::FrontToBack => match player_facing {
                Facing::North => Facing::West,
                Facing::West => Facing::South,
                Facing::South => Facing::East,
                Facing::East => Facing::North,
            },
        }
    }
}

/// The data row for one bbmodel block: its cache key, source file, cell footprint,
/// and collision policy. The geometry/texture come from `model_file` (read through
/// the asset roots, so a mod pack can override the art); this row carries only what
/// the source can't express. Rows live in `models.json` (a layered catalog like
/// `blocks.json`): known engine keys are `petramond:*`, mod additions are
/// `mod_id:*`, and bare keys error.
pub struct BlockModelDef {
    pub key: &'static str,
    pub model_file: &'static str,
    /// The block's footprint in CELLS `(sx, sy, sz)` — what the placed block
    /// OCCUPIES (placement gating, collision, selection, per-cell split).
    /// `(1, 1, 1)` is an ordinary single-cell block. How the model's geometry
    /// maps into it is [`fit`](Self::fit).
    pub cells: [u8; 3],
    pub collision: CollisionSpec,
    /// How the model turns to meet the placing player (workbench across the view,
    /// bed away from it).
    pub orientation: PlacementOrientation,
    /// How the authored geometry maps onto the footprint (see [`FitMode`]).
    pub fit: FitMode,
    /// Authored cube NAMES this row hides (applied after the cache load, so
    /// several rows can share one `.bbmodel` with different parts visible —
    /// the lit/unlit machine pattern). Empty for most rows.
    pub hidden_parts: &'static [&'static str],
}

/// How a model's authored geometry maps onto its footprint cell box.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FitMode {
    /// The default: uniform-scale the model's baked bounds to FILL the cell
    /// box (largest axis flush), X/Z centred, resting on the floor. Right for
    /// furniture that should exactly span its cells (workbench, bed, oven).
    #[default]
    Fill,
    /// Authored pixels map 1:1 onto the footprint grid — cell `(i,j,k)` IS
    /// authored `16i..16(i+1)`, no scaling, no centring. Geometry outside the
    /// box OVERHANGS visually (a hopper lip, a tray): it renders (assigned to
    /// the nearest footprint cell) but never extends collision, selection, or
    /// placement beyond the footprint — the standard cell clipping applies.
    /// Right for machines whose occupied space is smaller than their
    /// silhouette. Author the model resting at `y = 0` inside `0..16·cells`.
    Native,
}

/// One model row as written in `models.json`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawModelDef {
    key: String,
    model_file: String,
    cells: [u8; 3],
    orientation: PlacementOrientation,
    #[serde(default)]
    fit: FitMode,
    #[serde(default)]
    hidden_parts: Vec<String>,
}

#[derive(Deserialize)]
struct RawModelFile {
    models: Vec<RawModelDef>,
}

/// The loaded, id-ordered model def table. Loads exactly once; a missing or
/// inconsistent `models.json` fails loudly at startup.
fn defs() -> &'static [BlockModelDef] {
    static DEFS: LazyLock<&'static [BlockModelDef]> = LazyLock::new(|| {
        crate::registry::read_catalog("models.json", "block model", parse_layers).rows()
    });
    &DEFS
}

fn parse_layers(texts: &[&str]) -> Result<crate::registry::Catalog<BlockModelDef>, String> {
    crate::registry::load_catalog(
        texts,
        |text| serde_json::from_str::<RawModelFile>(text).map(|f| f.models),
        |r| &r.key,
        ENGINE_MODEL_KEYS,
        "block model",
        |r, id, names| {
            let hidden_parts: Vec<&'static str> = r
                .hidden_parts
                .into_iter()
                .map(|p| &*Box::leak(p.into_boxed_str()))
                .collect();
            Ok(BlockModelDef {
                key: names.name(id).expect("id resolved from this table"),
                model_file: Box::leak(r.model_file.into_boxed_str()),
                cells: r.cells,
                collision: CollisionSpec::FromModel,
                orientation: r.orientation,
                fit: r.fit,
                hidden_parts: Box::leak(hidden_parts.into_boxed_slice()),
            })
        },
    )
}

/// The registry row for `kind`.
#[inline]
pub fn def(kind: BlockModelKind) -> &'static BlockModelDef {
    &defs()[kind.0 as usize]
}
