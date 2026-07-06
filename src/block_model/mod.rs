//! Data-driven Blockbench (`.bbmodel`) BLOCKS — the chunk-meshed, world-placed kind,
//! the counterpart to the legacy atlas-cube blocks rather than to mobs.
//!
//! A bbmodel block is authored like a mob (cube elements + per-face UVs + an embedded
//! texture) but is a *block*: baked into the chunk mesh at remesh, lit at mesh-time, and
//! broken/collided with per cell exactly like a legacy block. The only thing it can't
//! share with the legacy packed path is its texturing — bbmodel faces carry arbitrary
//! sub-rectangle UVs, which the tile-packed vertex + fixed atlas can't express — so model
//! geometry rides a second, explicit-UV vertex stream in the chunk mesh and samples a
//! combined [`ModelAtlas`] instead of the block atlas.
//!
//! # Three layers
//!
//! 1. [`BlockModel`] — the CACHED parse: cube geometry (model space) + the decoded
//!    texture. This is the expensive step (`serde_json` + base64 + PNG decode), compiled
//!    once into a `.llblock` (see [`crate::asset_cache`]) and reused.
//! 2. [`ModelAtlas`] — all kinds' textures stacked into one sheet, with a per-kind UV
//!    transform, built once from the cached models. Shared by the off-thread mesher (UV
//!    remap) and the renderer (GPU upload).
//! 3. [`ModelInstance`] — the runtime bake derived from the cached model + its data row:
//!    the cell footprint, the cubes mapped into footprint space (with atlas UVs) and
//!    SPLIT per occupied cell, and each cell's collision + selection box. Cheap, so it
//!    lives outside the cache — tweaking the footprint or collision needs no cache bump.
//!
//! # Multi-block
//!
//! A model larger than one cell (the workbench is 2×2×1) declares its `cells` footprint
//! in its data row; the bake fits the model into that cell box (uniform scale, X/Z
//! centred, resting on the floor) and assigns each cube to the cell containing its
//! centre. In the world every footprint cell holds the block id; the per-chunk
//! `model_cells` map records authored cell offsets, and `model_facings` records placed
//! orientation. Placement gates the whole footprint clear, breaking any cell breaks the
//! group, and each cell meshes only its own cubes + collides with its own boxes.

use std::sync::LazyLock;

use glam::{Mat4, Quat, Vec3};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::asset_cache::CompiledAsset;
use crate::bbmodel::{euler_quat, face_corners, Model};
use crate::block::Aabb;
use crate::furnace::Facing;
use crate::mathh::IVec3;
use crate::mesh::face::Face;
use crate::mesh::SHADES;

mod atlas;
mod placement;
mod query;

pub use atlas::{atlas, particle_patch};
pub use placement::{
    base_from_cell, base_from_front_left_anchor, oriented_footprint_cells, placement_transform,
};
pub use query::{
    collision_boxes, collision_boxes_oriented, model_render_boxes, outline_bounds, ray_vs_model,
    selection_aabb, selection_aabb_oriented,
};

use placement::{oriented_cell_instance, placement_transform_fp};

/// Canonical bbmodel orientation: Blockbench/Minecraft model fronts face `-Z` (North).
/// Old model placements that predate per-cell facing read as this unrotated orientation.
pub const DEFAULT_MODEL_FACING: Facing = Facing::North;

/// One cube of a model: an axis-aligned box with a pivot + static rotation and a
/// per-face UV rect (in `Face::ALL` order; `None` = the face is omitted). The cached
/// [`BlockModel`] stores these in MODEL space; the runtime [`ModelInstance`] re-stores
/// them in footprint space with atlas-remapped UVs.
#[derive(Clone, Serialize, Deserialize)]
pub struct ModelCube {
    pub from: Vec3,
    pub to: Vec3,
    /// Pivot for this cube's static `rotation`.
    pub origin: Vec3,
    /// Static euler rotation in degrees, about `origin`.
    pub rotation: Vec3,
    /// Normalized `[u0, v0_top, u1, v1_bottom]` per face, `Face::ALL` order. Raw corner
    /// order (flips preserved), exactly as the mob model stores it.
    pub faces: [Option<[f32; 4]>; 6],
}

/// One Blockbench `display` transform (rotation degrees, translation in 1/16-block
/// units, scale multiplier, plus the optional rotation/scale pivots in block units) —
/// how the author wants the model posed in a given context (in-hand, in the GUI, …).
/// Cached in the `.llblock` so the held item + inventory icon can pose the model
/// exactly as designed. Default = identity (no display entry). A NEGATIVE scale
/// component is Blockbench's "mirror" checkbox (the sign is saved into the scale).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DisplayTransform {
    pub rotation: [f32; 3],
    pub translation: [f32; 3],
    pub scale: [f32; 3],
    pub rotation_pivot: [f32; 3],
    pub scale_pivot: [f32; 3],
}

impl Default for DisplayTransform {
    fn default() -> Self {
        DisplayTransform {
            rotation: [0.0; 3],
            translation: [0.0; 3],
            scale: [1.0; 3],
            rotation_pivot: [0.0; 3],
            scale_pivot: [0.0; 3],
        }
    }
}

impl DisplayTransform {
    /// The COMPLETE display transform `T(translation) · R(rotation) · S(scale)` with the
    /// pivot position-corrections, in BLOCK units — element-for-element the matrix
    /// Blockbench's display preview builds in `updateDisplayBase` (display_mode.js) for
    /// the right-hand / unmirrored contexts, so a pose renders exactly as authored.
    /// Model points are expected relative to the authored display pivot (the block
    /// centre — see [`BlockModel::display_pivot`] / [`ModelInstance::display_from_unit`]).
    /// Translation is authored in pixels (16 per block); the pivots in blocks. A zero
    /// scale component degrades to 0.001 exactly as Blockbench does; a negative one
    /// mirrors that axis (the authored "mirror" flag).
    pub fn base_matrix(&self) -> Mat4 {
        let rot = euler_quat(Vec3::from(self.rotation));
        let s = Vec3::from(self.scale);
        let guarded = |v: f32| if v == 0.0 { 0.001 } else { v };
        let scale = Vec3::new(guarded(s.x), guarded(s.y), guarded(s.z));
        let mut pos = Vec3::from(self.translation) / 16.0;
        let rp = Vec3::from(self.rotation_pivot);
        if rp != Vec3::ZERO {
            pos -= rot * rp - rp;
        }
        let sp = Vec3::from(self.scale_pivot);
        if sp != Vec3::ZERO {
            // Blockbench rotates the pivot FIRST, then damps it componentwise by
            // (1 - scale) — replicated verbatim, quirks included.
            pos += (rot * sp) * (Vec3::ONE - s);
        }
        Mat4::from_translation(pos) * Mat4::from_quat(rot) * Mat4::from_scale(scale)
    }

    fn parse(v: &Value) -> Self {
        let read = |key: &str, default: [f32; 3]| -> [f32; 3] {
            match v.get(key).and_then(Value::as_array) {
                Some(a) if a.len() == 3 => [
                    a[0].as_f64().unwrap_or(default[0] as f64) as f32,
                    a[1].as_f64().unwrap_or(default[1] as f64) as f32,
                    a[2].as_f64().unwrap_or(default[2] as f64) as f32,
                ],
                _ => default,
            }
        };
        DisplayTransform {
            rotation: read("rotation", [0.0; 3]),
            translation: read("translation", [0.0; 3]),
            scale: read("scale", [1.0; 3]),
            rotation_pivot: read("rotation_pivot", [0.0; 3]),
            scale_pivot: read("scale_pivot", [0.0; 3]),
        }
    }
}

/// The Blockbench `display` block: the per-context poses we use. Each defaults to
/// identity when the source omits it, so a model with no `display` still renders.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BlockDisplay {
    /// First-person right-hand pose — the HELD item.
    pub firstperson_righthand: DisplayTransform,
    /// Inventory / GUI pose — the slot ICON.
    pub gui: DisplayTransform,
    /// Third-person right-hand pose (cached for completeness; not yet wired).
    pub thirdperson_righthand: DisplayTransform,
    /// On-the-ground pose (cached for completeness; the dropped item keeps its own pose).
    pub ground: DisplayTransform,
}

impl BlockDisplay {
    /// Parse the `.bbmodel`'s `display` object (any context absent → identity).
    fn parse(root: &Value) -> Self {
        let d = root.get("display");
        let ctx = |name: &str| {
            d.and_then(|d| d.get(name))
                .map(DisplayTransform::parse)
                .unwrap_or_default()
        };
        BlockDisplay {
            firstperson_righthand: ctx("firstperson_righthand"),
            gui: ctx("gui"),
            thirdperson_righthand: ctx("thirdperson_righthand"),
            ground: ctx("ground"),
        }
    }
}

/// A compiled bbmodel block: cubes + the embedded RGBA texture, PLUS the collision
/// boxes and bounding box BAKED FROM THE GEOMETRY at compile time — all in MODEL space.
/// The cached, expensive-to-produce parse; placement/render/collision derive from this
/// (the cheap footprint fit + per-cell split happens at startup, see [`ModelInstance`]).
#[derive(Serialize, Deserialize)]
pub struct BlockModel {
    pub cubes: Vec<ModelCube>,
    pub texture_rgba: Vec<u8>,
    pub tex_w: u32,
    pub tex_h: u32,
    /// One AABB per SOLID cube (each posed by its static tilt), MODEL space — the block's
    /// collision SHAPE, hugging the actual cubes (legs, top, …) rather than one coarse
    /// box. Flat/degenerate cubes (a decorative plane, a locator point) contribute none.
    /// Computed + cached from the geometry; `CollisionSpec` chooses whether to use it.
    pub collision: Vec<Aabb>,
    /// The model's tight bounding box (MODEL space) over all cubes — the raycast/outline
    /// shape, so the black wireframe hugs the model's real extent. Cached from geometry.
    pub bounds: Aabb,
    /// The Blockbench `display` poses (hand / GUI / …), cached so the held item + slot
    /// icon orient the model exactly as authored rather than via a hardcoded angle.
    pub display: BlockDisplay,
    /// The point (MODEL space, authored pixel coords) the `display` poses transform
    /// about: the centre of the authored 16³ block cell. Blockbench pivots display
    /// previews there regardless of the model's real extent, so replicating a pose
    /// needs this exact point, not the geometric centre. Where it sits in authored
    /// coords depends on the format's grid: centred formats (bedrock_block, …) author
    /// x/z about 0 → pivot `(0, 8, 0)`; corner-grid formats (java_block) author
    /// `0..16` → pivot `(8, 8, 8)`.
    pub display_pivot: [f32; 3],
}

impl BlockModel {
    /// An empty placeholder (no cubes, a 1×1 transparent texture) so a parse failure
    /// degrades to "nothing drawn" instead of crashing.
    pub fn empty() -> Self {
        BlockModel {
            cubes: Vec::new(),
            texture_rgba: vec![0, 0, 0, 0],
            tex_w: 1,
            tex_h: 1,
            collision: Vec::new(),
            bounds: Aabb {
                min: [0.0; 3],
                max: [1.0; 3],
            },
            display: BlockDisplay::default(),
            display_pivot: [0.0, 8.0, 0.0],
        }
    }

    /// Keep the cube geometry + texture from a parsed mob-frontend [`Model`] and BAKE
    /// the collision boxes + bounding box from that geometry. A block has no
    /// animations, but authored GROUP rotations are part of the rest pose Blockbench
    /// displays (the bed is authored under a 90°-turned group) — they are baked into
    /// each cube here (composed rotation + shifted box, an exact equivalence) so the
    /// compiled model is WYSIWYG with the Blockbench scene. Dropping them was the
    /// 2026-07-05 "held bed 90° off" bug.
    fn from_model(m: &Model) -> Self {
        let rest = m.rest_pose();
        let cubes: Vec<ModelCube> = m
            .cubes
            .iter()
            .map(|c| {
                let pose = rest.get(c.bone).copied().unwrap_or(Mat4::IDENTITY);
                if pose.abs_diff_eq(Mat4::IDENTITY, 1e-6) {
                    return ModelCube {
                        from: c.from,
                        to: c.to,
                        origin: c.origin,
                        rotation: c.rotation,
                        faces: c.faces,
                    };
                }
                // Fold the bone-chain pose `A` into the cube: the posed cube
                // `A · (T(o)·Rc·T(−o))` equals a plain cube with rotation
                // `R = A_rot·Rc` about `o' = A(o)` and its box shifted by `o' − o`
                // (derivation: `R(p + s − o') + o' = A_rot·Rc·p + A(o) − A_rot·Rc·o`
                // with `s = o' − o`). Faces stay attached to the cube's local axes,
                // exactly like an authored static tilt.
                let rot = Quat::from_mat4(&pose) * euler_quat(c.rotation);
                let (ex, ey, ez) = rot.to_euler(glam::EulerRot::XYZ);
                let origin = pose.transform_point3(c.origin);
                let shift = origin - c.origin;
                ModelCube {
                    from: c.from + shift,
                    to: c.to + shift,
                    origin,
                    rotation: Vec3::new(ex.to_degrees(), ey.to_degrees(), ez.to_degrees()),
                    faces: c.faces,
                }
            })
            .collect();
        // Collision = one posed AABB per SOLID cube (skip flat/degenerate — a zero-extent
        // cube is decoration, not a wall). Bounds = the whole model's tight box.
        let mut collision = Vec::new();
        let mut bmn = Vec3::splat(f32::INFINITY);
        let mut bmx = Vec3::splat(f32::NEG_INFINITY);
        for c in &cubes {
            let (mn, mx) = posed_cube_bounds(c);
            bmn = bmn.min(mn);
            bmx = bmx.max(mx);
            if (mx - mn).min_element() > 1e-4 {
                collision.push(Aabb {
                    min: mn.to_array(),
                    max: mx.to_array(),
                });
            }
        }
        let bounds = if bmn.is_finite() {
            Aabb {
                min: bmn.to_array(),
                max: bmx.to_array(),
            }
        } else {
            Aabb {
                min: [0.0; 3],
                max: [1.0; 3],
            }
        };
        BlockModel {
            cubes,
            texture_rgba: m.texture_rgba.clone(),
            tex_w: m.tex_w,
            tex_h: m.tex_h,
            collision,
            bounds,
            // `from_model` has only the parsed geometry; `compile` fills the display
            // poses + pivot from the raw JSON (the mob frontend drops them).
            display: BlockDisplay::default(),
            display_pivot: [0.0, 8.0, 0.0],
        }
    }
}

impl CompiledAsset for BlockModel {
    /// `LLBLK` — the compiled bbmodel-block container (geometry + texture + baked
    /// collision/bounds), distinct from the mob `LLMOB` so the two never alias.
    const MAGIC: [u8; 8] = *b"LLBLK\0\0\0";
    /// v6: model-space cubes (per-face UV, group REST POSES baked in) + RGBA texture +
    /// baked per-cube collision + bounding box + the FULL Blockbench `display` poses
    /// (incl. rotation/scale pivots) + the authored display pivot. (v1 had no
    /// collision/bounds; v2 no display; v3 predates the multi-texture sheet; v4 the
    /// display pivots; v5 dropped group rest poses; each bump rebuilds stale caches.)
    /// v7: the shared loader bakes element `inflate` into the cube box.
    const FORMAT_VERSION: u32 = 7;
    const SUBDIR: &'static str = "models";
    const EXTENSION: &'static str = "llblock";

    /// Compile = parse the authored `.bbmodel` via the shared mob frontend (geometry +
    /// texture), then parse the `display` poses from the raw JSON (the mob frontend
    /// drops them — only a block needs them).
    fn compile(source: &[u8]) -> Result<Self, String> {
        let src = std::str::from_utf8(source).map_err(|e| format!("bbmodel utf-8: {e}"))?;
        let mut model = BlockModel::from_model(&Model::load(src)?);
        let root: Value = serde_json::from_str(src).map_err(|e| format!("json: {e}"))?;
        model.display = BlockDisplay::parse(&root);
        // The display pivot follows the authoring grid: java_block authors 0..16
        // (corner grid), every other Blockbench format centres x/z about 0.
        let corner_grid = root
            .get("meta")
            .and_then(|m| m.get("model_format"))
            .and_then(Value::as_str)
            == Some("java_block");
        model.display_pivot = if corner_grid {
            [8.0, 8.0, 8.0]
        } else {
            [0.0, 8.0, 0.0]
        };
        Ok(model)
    }
}

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
    "llama:furniture_workbench",
    "llama:bucket",
    "llama:water_bucket",
    "llama:bed_frame",
    "llama:bed",
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
/// `blocks.json`): known engine keys are `llama:*`, mod additions are
/// `mod_id:*`, and bare keys error.
pub struct BlockModelDef {
    pub key: &'static str,
    pub model_file: &'static str,
    /// The block's footprint in CELLS `(sx, sy, sz)` — the model is fitted into this
    /// cell box and split across it. `(1, 1, 1)` is an ordinary single-cell block.
    pub cells: [u8; 3],
    pub collision: CollisionSpec,
    /// How the model turns to meet the placing player (workbench across the view,
    /// bed away from it).
    pub orientation: PlacementOrientation,
}

/// One model row as written in `models.json`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawModelDef {
    key: String,
    model_file: String,
    cells: [u8; 3],
    orientation: PlacementOrientation,
}

#[derive(Deserialize)]
struct RawModelFile {
    models: Vec<RawModelDef>,
}

/// The loaded, id-ordered model def table. Loads exactly once; a missing or
/// inconsistent `models.json` fails loudly at startup.
fn defs() -> &'static [BlockModelDef] {
    static DEFS: LazyLock<&'static [BlockModelDef]> = LazyLock::new(|| {
        let layers = crate::assets::read_layers("models.json");
        if layers.is_empty() {
            panic!(
                "models.json not found (searched {:?}); the game cannot run without its model table",
                crate::assets::candidate_paths("models.json")
            );
        }
        let texts: Vec<&str> = layers.iter().map(|(s, _)| s.as_str()).collect();
        Box::leak(
            parse_layers(&texts)
                .unwrap_or_else(|e| panic!("models.json: {e}"))
                .into_boxed_slice(),
        )
    });
    &DEFS
}

fn parse_layers(texts: &[&str]) -> Result<Vec<BlockModelDef>, String> {
    // Same catalog contract as blocks/items/sounds: merge rows by key, engine
    // keys keep their frozen ids, namespaced keys register fresh ids.
    let mut merged: Vec<RawModelDef> = Vec::new();
    let mut layer_keys: Vec<Vec<String>> = Vec::new();
    for (li, text) in texts.iter().enumerate() {
        let raw: RawModelFile =
            serde_json::from_str(text).map_err(|e| format!("layer #{li}: invalid JSON: {e}"))?;
        layer_keys.push(raw.models.iter().map(|r| r.key.clone()).collect());
        for r in raw.models {
            match merged.iter_mut().find(|m| m.key == r.key) {
                Some(slot) => *slot = r,
                None => merged.push(r),
            }
        }
    }
    let names = crate::registry::NameTable::build(ENGINE_MODEL_KEYS, &layer_keys, "block model")?;
    let mut rows: Vec<Option<BlockModelDef>> = (0..names.len()).map(|_| None).collect();
    for r in merged {
        let id = names
            .id(&r.key)
            .ok_or_else(|| format!("unregistered block model '{}'", r.key))?;
        rows[id as usize] = Some(BlockModelDef {
            key: Box::leak(r.key.into_boxed_str()),
            model_file: Box::leak(r.model_file.into_boxed_str()),
            cells: r.cells,
            collision: CollisionSpec::FromModel,
            orientation: r.orientation,
        });
    }
    rows.into_iter()
        .enumerate()
        .map(|(id, row)| {
            row.ok_or_else(|| {
                format!(
                    "missing row for block model '{}'",
                    names.name(id as u8).unwrap_or("?")
                )
            })
        })
        .collect()
}

/// The registry row for `kind`.
#[inline]
pub fn def(kind: BlockModelKind) -> &'static BlockModelDef {
    &defs()[kind.0 as usize]
}

/// Every kind's compiled [`BlockModel`], indexed by raw kind id — the cached parse,
/// precached once on first use (compiling each `.bbmodel` → `.llblock` on a miss, else
/// fast-loading the `.llblock`).
static MODELS: LazyLock<Vec<BlockModel>> = LazyLock::new(|| {
    all()
        .iter()
        .map(|&k| {
            let d = def(k);
            let Some((src, _)) = crate::assets::read_bytes(d.model_file) else {
                log::error!(
                    "block model '{}' not found in the asset roots",
                    d.model_file
                );
                return BlockModel::empty();
            };
            crate::asset_cache::load_or_compile::<BlockModel>(d.key, &src).unwrap_or_else(|e| {
                log::error!("block model precache failed for {k:?}: {e}");
                BlockModel::empty()
            })
        })
        .collect()
});

// ---------------------------------------------------------------------------------
// Runtime instance: footprint, per-cell split, collision, selection
// ---------------------------------------------------------------------------------

/// One occupied cell of a model's footprint: which cubes render from it, and its
/// cell-local collision + selection box.
pub struct CellInstance {
    /// Offset of this cell from the footprint origin, `0..footprint` per axis.
    pub offset: [u8; 3],
    /// Indices into [`ModelInstance::cubes`] of the cubes assigned to this cell (by
    /// centre). The geometry is positioned in FOOTPRINT space, so the mesher places it
    /// at `origin_world + cube` regardless of which cell emits it.
    pub cubes: Vec<u32>,
    /// Cell-local collision boxes (`0..1`) — the model's per-cube collision SHAPE clipped
    /// to this cell, so the player collides with the actual legs/top, not one coarse box.
    pub collision: Vec<Aabb>,
    /// Cell-local selection/targeting box (`0..1`): the bbox of the cube geometry
    /// OVERLAPPING this cell, so the raycast targets the cell where the model actually is
    /// (the drawn outline is the whole-model box — see [`ModelInstance::bounds`]).
    pub selection_min: [f32; 3],
    pub selection_max: [f32; 3],
}

/// One occupied authored cell after applying a placement facing: collision/selection are
/// expressed in the rotated world voxel's local coordinates, but keyed by the authored
/// offset stored in the chunk.
pub struct OrientedCellInstance {
    pub offset: [u8; 3],
    pub collision: Vec<Aabb>,
    pub selection_min: [f32; 3],
    pub selection_max: [f32; 3],
}

/// One ready-to-stream vertex of a baked model cell: position in FOOTPRINT space already
/// transformed through the cube's static rotation AND the placement facing (so the mesher
/// only translates by the world base), the atlas UV, and the directional face shade
/// (pre-light). The mesher folds in cell light × warm tint per placement — see
/// [`ModelCellTemplate`].
#[derive(Copy, Clone)]
pub struct ModelTemplateVertex {
    pub pos: Vec3,
    pub uv: [f32; 2],
    pub shade: f32,
}

/// The fully baked geometry of one occupied cell at one facing: the exact vertices +
/// indices the mesher emits, with every per-cube matrix, quaternion, face-bias, and
/// degenerate-face decision already resolved at startup. Meshing a placed cell is then a
/// translate-by-base + scale-shade-by-light + copy — no `Mat4`/quat/trig per remesh.
pub struct ModelCellTemplate {
    pub verts: Vec<ModelTemplateVertex>,
    /// Quad indices relative to the cell's first vertex (`0,1,2, 0,2,3` per face).
    pub indices: Vec<u32>,
}

/// The runtime bake of a model kind: its footprint, the cubes in footprint space with
/// atlas-remapped UVs, and the per-cell split. Derived from the cached [`BlockModel`] +
/// its data row + the [`ModelAtlas`].
pub struct ModelInstance {
    pub footprint: [u8; 3],
    /// Cubes in FOOTPRINT space (coords `0..footprint`, 1 unit = 1 world cell), with
    /// faces already remapped into the model-atlas sheet.
    pub cubes: Vec<ModelCube>,
    pub cells: Vec<CellInstance>,
    /// The whole model's tight bounding box in FOOTPRINT space (relative to the
    /// footprint origin) — the raycast outline, drawn as ONE box hugging the model's real
    /// extent rather than a per-cell cube. Baked from geometry (the cached `bounds`).
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    /// One FOOTPRINT-space posed AABB per cube (the whole model) — the surfaces the
    /// break-crack overlay paints over, so the crack lands on the model (each leg / the
    /// top, the whole piece) instead of floating in the cell's air. Positioned by the
    /// caller at the footprint-origin world cell.
    pub cube_boxes: Vec<Aabb>,
    /// Per-facing collision/selection data. Indexed by [`Facing::to_u8`], and each list
    /// is still keyed by authored cell offset.
    pub oriented_cells: [Vec<OrientedCellInstance>; 4],
    /// Per-facing, per-cell baked render geometry — the chunk-mesher's hot path. Indexed
    /// by [`Facing::to_u8`] then by the SAME order as [`Self::cells`] (use
    /// [`Self::cell_template`]). All the static work (cube rotation, placement rotation,
    /// face bias, degenerate-face culling, atlas UVs, directional shade) is resolved here
    /// once so a remesh just translates + lights the verts.
    pub oriented_render: [Vec<ModelCellTemplate>; 4],
    /// Maps the CENTRED-UNIT item space (the `build_block_model_item` bake: footprint
    /// centred on the origin, largest axis spanning ±0.5) back to the model's AUTHORED
    /// display space in blocks — origin at the authored display pivot, 1 unit = 16
    /// authored pixels. This undoes the placement fit (floor-rest, centring, fill
    /// scale) so a Blockbench `display` pose ([`DisplayTransform::base_matrix`])
    /// composes about the exact geometry Blockbench posed, and renders identically.
    pub display_from_unit: Mat4,
}

impl ModelInstance {
    /// The cell data for `offset`, or `None` if that cell isn't part of the footprint.
    #[inline]
    pub fn cell(&self, offset: [u8; 3]) -> Option<&CellInstance> {
        self.cells.iter().find(|c| c.offset == offset)
    }

    /// The oriented cell data for `offset` under `facing`.
    #[inline]
    pub fn oriented_cell(&self, offset: [u8; 3], facing: Facing) -> Option<&OrientedCellInstance> {
        self.oriented_cells[facing.to_u8() as usize]
            .iter()
            .find(|c| c.offset == offset)
    }

    /// The baked render geometry for `offset` under `facing`, or `None` if that cell isn't
    /// part of the footprint. The chunk mesher's only model-geometry lookup.
    #[inline]
    pub fn cell_template(&self, offset: [u8; 3], facing: Facing) -> Option<&ModelCellTemplate> {
        let idx = self.cells.iter().position(|c| c.offset == offset)?;
        Some(&self.oriented_render[facing.to_u8() as usize][idx])
    }

    fn build(kind: BlockModelKind) -> Self {
        let m = &MODELS[kind.0 as usize];
        let d = def(kind);
        let footprint = d.cells.map(|c| c.max(1));
        let at = atlas();

        // --- Fit the model into the footprint cell box: uniform scale (no stretch),
        // X/Z centred, resting on the floor in Y. Uses the BAKED posed bounds so the fit,
        // the outline, and the collision all agree on the model's extent. ---
        let (mn, mx) = (Vec3::from(m.bounds.min), Vec3::from(m.bounds.max));
        let extent = mx - mn;
        let fp = Vec3::new(
            footprint[0] as f32,
            footprint[1] as f32,
            footprint[2] as f32,
        );
        // World units per model unit: the tightest axis sets a uniform scale so the
        // model fills its largest footprint axis and keeps its proportions.
        let per_unit = [extent.x / fp.x, extent.y / fp.y, extent.z / fp.z]
            .into_iter()
            .fold(f32::MIN_POSITIVE, f32::max);
        let scale = 1.0 / per_unit;
        // Centre on X/Z within the footprint; floor on Y.
        let span = extent * scale;
        let lo = Vec3::new((fp.x - span.x) * 0.5, 0.0, (fp.z - span.z) * 0.5);
        let to_fp = |v: Vec3| lo + (v - mn) * scale;
        // A model-space AABB → footprint space (uniform scale + translate keeps it axis-
        // aligned, so transforming the two corners suffices).
        let to_fp_box = |b: &Aabb| Aabb {
            min: to_fp(Vec3::from(b.min)).to_array(),
            max: to_fp(Vec3::from(b.max)).to_array(),
        };

        // --- Cubes in footprint space, UVs remapped into the model atlas. ---
        let cubes: Vec<ModelCube> = m
            .cubes
            .iter()
            .map(|c| ModelCube {
                from: to_fp(c.from),
                to: to_fp(c.to),
                origin: to_fp(c.origin),
                rotation: c.rotation,
                faces: c.faces.map(|f| {
                    f.map(|[u0, v0, u1, v1]| {
                        let [au0, av0] = at.remap(kind, [u0, v0]);
                        let [au1, av1] = at.remap(kind, [u1, v1]);
                        [au0, av0, au1, av1]
                    })
                }),
            })
            .collect();

        // --- The collision SHAPE (footprint space): the model's baked per-cube boxes,
        // split per cell. A cube spanning two cells (the full-width table top) is split
        // into both. ---
        let footprint_collision: Vec<Aabb> = match d.collision {
            CollisionSpec::FromModel => m.collision.iter().map(&to_fp_box).collect(),
        };
        // Per-cube footprint AABBs (posed), for the per-cell targeting boxes.
        let cube_boxes: Vec<Aabb> = cubes
            .iter()
            .map(|c| {
                let (mn, mx) = posed_cube_bounds(c);
                Aabb {
                    min: mn.to_array(),
                    max: mx.to_array(),
                }
            })
            .collect();

        // --- Split per occupied cell. ---
        let mut cells = Vec::new();
        for dz in 0..footprint[2] {
            for dy in 0..footprint[1] {
                for dx in 0..footprint[0] {
                    let offset = [dx, dy, dz];
                    let o = Vec3::new(dx as f32, dy as f32, dz as f32);
                    // Cubes whose centre falls in this cell render from it (once each).
                    let cube_idx: Vec<u32> = cubes
                        .iter()
                        .enumerate()
                        .filter(|(_, c)| cell_of((c.from + c.to) * 0.5, footprint) == offset)
                        .map(|(i, _)| i as u32)
                        .collect();
                    // Collision: every collision box overlapping this cell, clipped local.
                    let collision: Vec<Aabb> = footprint_collision
                        .iter()
                        .filter_map(|b| clip_to_cell(b, o))
                        .collect();
                    // Targeting box: the union of cube geometry overlapping this cell.
                    let sel = union_clip_to_cell(&cube_boxes, o);
                    let (selection_min, selection_max) = match sel {
                        Some(s) => (s.min, s.max),
                        None => ([0.0; 3], [0.0; 3]),
                    };
                    // Keep a cell only if it renders, collides, or can be targeted — so an
                    // empty corner of the footprint isn't a phantom solid.
                    if cube_idx.is_empty() && collision.is_empty() && sel.is_none() {
                        continue;
                    }
                    cells.push(CellInstance {
                        offset,
                        cubes: cube_idx,
                        collision,
                        selection_min,
                        selection_max,
                    });
                }
            }
        }

        let bounds = to_fp_box(&m.bounds);
        let oriented_cells = std::array::from_fn(|i| {
            let facing = Facing::from_u8(i as u8);
            cells
                .iter()
                .map(|cell| oriented_cell_instance(cell, footprint, facing))
                .collect()
        });

        // Bake the per-facing render geometry once. `placement_transform` with a ZERO base
        // gives the facing's rotation + footprint shift; the mesher adds the integer world
        // base at remesh. All the per-cube/per-face math the mesher used to redo every
        // remesh (quaternions, matrix products, face bias, degenerate-face culling) is
        // resolved here.
        let oriented_render = std::array::from_fn(|i| {
            let facing = Facing::from_u8(i as u8);
            // Explicit local footprint, NOT placement_transform(kind, ..): this runs inside
            // the INSTANCES LazyLock init, so resolving footprint(kind) would deadlock.
            let base_xform = placement_transform_fp(IVec3::ZERO, footprint, facing);
            cells
                .iter()
                .map(|cell| bake_cell_template(base_xform, &cubes, &cell.cubes))
                .collect()
        });

        // Centred-unit item space → authored display space (blocks about the display
        // pivot): invert the item bake's centring (`p_fp = p_unit·uspan + fp/2`), then
        // the footprint fit (`p_px = mn + (p_fp − lo)·per_unit`), then rebase on the
        // pivot in blocks. Uniform scale + translation, folded into one matrix.
        let display_from_unit = {
            let uspan = fp.max_element().max(1.0);
            let pivot = Vec3::from(m.display_pivot);
            let k = uspan * per_unit / 16.0;
            let offset = (mn + (fp * 0.5 - lo) * per_unit - pivot) / 16.0;
            Mat4::from_translation(offset) * Mat4::from_scale(Vec3::splat(k))
        };

        ModelInstance {
            footprint,
            cubes,
            cells,
            bounds_min: bounds.min,
            bounds_max: bounds.max,
            cube_boxes,
            oriented_cells,
            oriented_render,
            display_from_unit,
        }
    }
}

/// Bake one cell's render geometry at a given facing into a [`ModelCellTemplate`]. Mirrors
/// the order the chunk mesher used to emit in (cube-by-cube in `cube_idx` order, then
/// `Face::ALL` order), so the streamed geometry is unchanged — only the work moves to
/// startup. `base_xform` is the facing transform with a ZERO base (see [`ModelInstance::build`]).
fn bake_cell_template(
    base_xform: Mat4,
    cubes: &[ModelCube],
    cube_idx: &[u32],
) -> ModelCellTemplate {
    let mut verts = Vec::new();
    let mut indices = Vec::new();
    for &ci in cube_idx {
        let cube = &cubes[ci as usize];
        let m = base_xform
            * Mat4::from_translation(cube.origin)
            * Mat4::from_quat(euler_quat(cube.rotation))
            * Mat4::from_translation(-cube.origin);
        for (slot, face) in Face::ALL.into_iter().enumerate() {
            let Some(uv) = cube.faces[slot] else { continue };
            let Some(bias) = render_face_bias(cube, cubes, face) else {
                continue;
            };
            push_template_face(
                &mut verts,
                &mut indices,
                m,
                face,
                cube.from,
                cube.to,
                bias,
                uv,
                SHADES[face.shade_idx() as usize],
            );
        }
    }
    ModelCellTemplate { verts, indices }
}

/// Append one textured cube face to a cell template. Cell light and warm tint are
/// applied later by the mesher.
#[allow(clippy::too_many_arguments)]
fn push_template_face(
    verts: &mut Vec<ModelTemplateVertex>,
    indices: &mut Vec<u32>,
    m: Mat4,
    face: Face,
    from: Vec3,
    to: Vec3,
    bias: Vec3,
    uv: [f32; 4],
    shade: f32,
) {
    let local = face_corners(face, from, to);
    let p: [Vec3; 4] = [
        m.transform_point3(Vec3::from(local[0]) + bias),
        m.transform_point3(Vec3::from(local[1]) + bias),
        m.transform_point3(Vec3::from(local[2]) + bias),
        m.transform_point3(Vec3::from(local[3]) + bias),
    ];
    if (p[1] - p[0]).cross(p[3] - p[0]).length_squared() < 1e-9 {
        return;
    }
    // UV rect is [u0, v0_top, u1, v1_bottom]; assign per `quad_box` corner order
    // (p0 bottom-left, p1 bottom-right, p2 top-right, p3 top-left).
    let [u0, v0, u1, v1] = uv;
    let corner_uv = [[u0, v1], [u1, v1], [u1, v0], [u0, v0]];
    let start = verts.len() as u32;
    for i in 0..4 {
        verts.push(ModelTemplateVertex {
            pos: p[i],
            uv: corner_uv[i],
            shade,
        });
    }
    indices.extend_from_slice(&[start, start + 1, start + 2, start, start + 2, start + 3]);
}

/// Every kind's runtime [`ModelInstance`], indexed by `kind as usize`.
static INSTANCES: LazyLock<Vec<ModelInstance>> =
    LazyLock::new(|| all().iter().map(|&k| ModelInstance::build(k)).collect());

/// This kind's runtime instance (footprint + per-cell geometry/collision/selection).
#[inline]
pub fn instance(kind: BlockModelKind) -> &'static ModelInstance {
    &INSTANCES[kind.0 as usize]
}

/// The block's footprint in cells `(sx, sy, sz)`.
#[inline]
pub fn footprint(kind: BlockModelKind) -> [u8; 3] {
    instance(kind).footprint
}

/// The Blockbench `display` poses for `kind` (cached in the `.llblock`) — the held item
/// reads `firstperson_righthand`, the inventory icon reads `gui`.
#[inline]
pub fn display(kind: BlockModelKind) -> &'static BlockDisplay {
    &MODELS[kind.0 as usize].display
}

// ---------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------

/// Cube-space thickness below which a cube is treated as an authored plane. Blockbench
/// lets artists use zero-thickness cubes for decals/details; emitting both collapsed
/// opposite faces in our depth-tested pass creates z fighting.
const FLAT_FACE_EPS: f32 = 1e-4;
/// Tiny local-space offset applied to an emitted flat-cube surface so it sits just above
/// the supporting face it was authored onto (paper on the tabletop, plans on the back).
/// Keep it visibly flat, but large enough to survive depth precision at distance.
const FLAT_FACE_BIAS: f32 = 1.0 / 64.0;
/// Maximum gap, in footprint/world-cell units, at which a solid overlapping cube is
/// considered the surface a flat detail was authored onto.
const FLAT_SUPPORT_MAX_GAP: f32 = 0.125;

/// Whether `face` should be emitted for `cube`, plus a local-space positional bias to
/// apply to each corner before the cube's static rotation. Non-flat cubes return a zero
/// bias. A cube flat on exactly one axis keeps only one of the collapsed opposite faces,
/// preferring the face that points away from the nearest overlapping solid support.
/// Cubes flat on two or three axes have no renderable area.
pub(crate) fn render_face_bias(
    cube: &ModelCube,
    all_cubes: &[ModelCube],
    face: Face,
) -> Option<Vec3> {
    let extent = (cube.to - cube.from).abs();
    let flat = [
        extent.x <= FLAT_FACE_EPS,
        extent.y <= FLAT_FACE_EPS,
        extent.z <= FLAT_FACE_EPS,
    ];
    let flat_count = flat.into_iter().filter(|&v| v).count();
    if flat_count == 0 {
        return Some(Vec3::ZERO);
    }
    if flat_count >= 2 {
        return None;
    }

    let (axis, neg, pos) = if flat[0] {
        (0, Face::NegX, Face::PosX)
    } else if flat[1] {
        (1, Face::NegY, Face::PosY)
    } else {
        (2, Face::NegZ, Face::PosZ)
    };
    if face != neg && face != pos {
        return None;
    }

    let preferred = supported_flat_face(cube, all_cubes, axis, neg, pos).unwrap_or(pos);
    let fallback = if preferred == pos { neg } else { pos };
    let keep = match (
        cube.faces[face_slot(preferred)].is_some(),
        cube.faces[face_slot(fallback)].is_some(),
    ) {
        (true, _) => preferred,
        (false, true) => fallback,
        (false, false) => return None,
    };
    (face == keep).then_some(face_normal(keep) * FLAT_FACE_BIAS)
}

#[inline]
fn face_slot(face: Face) -> usize {
    Face::ALL.iter().position(|&f| f == face).unwrap_or(0)
}

#[inline]
fn face_normal(face: Face) -> Vec3 {
    match face {
        Face::PosX => Vec3::X,
        Face::NegX => Vec3::NEG_X,
        Face::PosY => Vec3::Y,
        Face::NegY => Vec3::NEG_Y,
        Face::PosZ => Vec3::Z,
        Face::NegZ => Vec3::NEG_Z,
    }
}

/// Pick the side of a zero-thickness cube that points away from the closest overlapping
/// non-flat support cube. For example, a paper sitting on a tabletop keeps +Y; a poster
/// sitting on the front of a back board keeps -Z. If no plausible support is found, the
/// caller falls back to Blockbench's positive face.
fn supported_flat_face(
    cube: &ModelCube,
    all_cubes: &[ModelCube],
    axis: usize,
    neg: Face,
    pos: Face,
) -> Option<Face> {
    let plane = (cube.from[axis] + cube.to[axis]) * 0.5;
    let mut neg_gap = f32::INFINITY;
    let mut pos_gap = f32::INFINITY;

    for other in all_cubes {
        if std::ptr::eq(other, cube) {
            continue;
        }
        let other_extent = (other.to - other.from).abs();
        if other_extent[axis] <= FLAT_FACE_EPS || other_extent.min_element() <= FLAT_FACE_EPS {
            continue;
        }
        if !flat_support_overlaps(cube, other, axis) {
            continue;
        }

        let omin = other.from[axis].min(other.to[axis]);
        let omax = other.from[axis].max(other.to[axis]);
        if omax <= plane + FLAT_FACE_EPS {
            neg_gap = neg_gap.min((plane - omax).max(0.0));
        }
        if omin >= plane - FLAT_FACE_EPS {
            pos_gap = pos_gap.min((omin - plane).max(0.0));
        }
    }

    let neg_supported = neg_gap <= FLAT_SUPPORT_MAX_GAP;
    let pos_supported = pos_gap <= FLAT_SUPPORT_MAX_GAP;
    match (neg_supported, pos_supported) {
        (true, true) if neg_gap <= pos_gap => Some(pos),
        (true, true) => Some(neg),
        (true, false) => Some(pos),
        (false, true) => Some(neg),
        (false, false) => None,
    }
}

fn flat_support_overlaps(flat: &ModelCube, support: &ModelCube, flat_axis: usize) -> bool {
    for axis in 0..3 {
        if axis == flat_axis {
            continue;
        }
        let amin = flat.from[axis].min(flat.to[axis]);
        let amax = flat.from[axis].max(flat.to[axis]);
        let bmin = support.from[axis].min(support.to[axis]);
        let bmax = support.from[axis].max(support.to[axis]);
        if amax <= bmin + FLAT_FACE_EPS || bmax <= amin + FLAT_FACE_EPS {
            return false;
        }
    }
    true
}

/// Bounds of ONE cube POSED by its static tilt (its 8 corners rotated about its pivot),
/// so a rotated cube's true extent is captured. Works in any space (model or footprint).
fn posed_cube_bounds(c: &ModelCube) -> (Vec3, Vec3) {
    let tilt = Mat4::from_translation(c.origin)
        * Mat4::from_quat(euler_quat(c.rotation))
        * Mat4::from_translation(-c.origin);
    let mut mn = Vec3::splat(f32::INFINITY);
    let mut mx = Vec3::splat(f32::NEG_INFINITY);
    for corner in box_corners(c.from, c.to) {
        let p = tilt.transform_point3(corner);
        mn = mn.min(p);
        mx = mx.max(p);
    }
    (mn, mx)
}

/// The cell-local union bbox of `boxes` clipped to the unit cell at `offset`, or `None`
/// if none reach into it. Used for a cell's targeting box (the geometry overlapping it).
fn union_clip_to_cell(boxes: &[Aabb], offset: Vec3) -> Option<Aabb> {
    let mut mn = [f32::INFINITY; 3];
    let mut mx = [f32::NEG_INFINITY; 3];
    let mut any = false;
    for b in boxes {
        if let Some(c) = clip_to_cell(b, offset) {
            any = true;
            for i in 0..3 {
                mn[i] = mn[i].min(c.min[i]);
                mx[i] = mx[i].max(c.max[i]);
            }
        }
    }
    any.then_some(Aabb { min: mn, max: mx })
}

/// The 8 corners of box `[from, to]`.
fn box_corners(from: Vec3, to: Vec3) -> [Vec3; 8] {
    [
        Vec3::new(from.x, from.y, from.z),
        Vec3::new(to.x, from.y, from.z),
        Vec3::new(from.x, to.y, from.z),
        Vec3::new(to.x, to.y, from.z),
        Vec3::new(from.x, from.y, to.z),
        Vec3::new(to.x, from.y, to.z),
        Vec3::new(from.x, to.y, to.z),
        Vec3::new(to.x, to.y, to.z),
    ]
}

/// The footprint cell (clamped into `0..footprint`) containing footprint-space point `p`.
fn cell_of(p: Vec3, footprint: [u8; 3]) -> [u8; 3] {
    [
        (p.x.floor() as i32).clamp(0, footprint[0] as i32 - 1) as u8,
        (p.y.floor() as i32).clamp(0, footprint[1] as i32 - 1) as u8,
        (p.z.floor() as i32).clamp(0, footprint[2] as i32 - 1) as u8,
    ]
}

/// Clip footprint-space box `b` to the unit cell at `offset`, returning it in CELL-LOCAL
/// `0..1` coordinates, or `None` if the box doesn't reach into that cell.
fn clip_to_cell(b: &Aabb, offset: Vec3) -> Option<Aabb> {
    let mut min = [0.0f32; 3];
    let mut max = [0.0f32; 3];
    for i in 0..3 {
        let lo = (b.min[i] - offset[i]).max(0.0);
        let hi = (b.max[i] - offset[i]).min(1.0);
        if hi - lo <= 1e-4 {
            return None;
        }
        min[i] = lo;
        max[i] = hi;
    }
    Some(Aabb { min, max })
}

#[cfg(test)]
mod tests {
    /// Test helper: a kind's .bbmodel source read through the asset roots.
    fn model_bytes(kind: BlockModelKind) -> Vec<u8> {
        let file = def(kind).model_file;
        crate::assets::read_bytes(file)
            .unwrap_or_else(|| panic!("bbmodel '{file}' not found"))
            .0
    }

    use super::*;

    const WB: BlockModelKind = BlockModelKind::FurnitureWorkbench;

    #[test]
    fn workbench_compiles_with_geometry_and_texture() {
        let m = BlockModel::compile(&model_bytes(WB)).expect("compiles");
        assert!(!m.cubes.is_empty());
        assert_eq!((m.tex_w, m.tex_h), (128, 128));
        assert_eq!(m.texture_rgba.len(), 128 * 128 * 4);
    }

    #[test]
    fn every_registered_model_compiles_with_geometry_and_texture() {
        // A bad bbmodel export degrades to an EMPTY model at runtime (log +
        // invisible), so a compile failure must be caught here instead.
        for &kind in all() {
            let m = BlockModel::compile(&model_bytes(kind))
                .unwrap_or_else(|e| panic!("{kind:?} fails to compile: {e}"));
            assert!(!m.cubes.is_empty(), "{kind:?} has no geometry");
            assert_eq!(
                m.texture_rgba.len(),
                (m.tex_w * m.tex_h * 4) as usize,
                "{kind:?} texture size mismatch"
            );
            assert!(m.tex_w > 0 && m.tex_h > 0, "{kind:?} has no texture");
        }
    }

    #[test]
    fn footprint_is_two_by_two_by_one() {
        assert_eq!(footprint(WB), [2, 2, 1], "authored 2 wide, 2 tall, 1 long");
    }

    #[test]
    fn flat_model_cubes_emit_one_biased_surface_face() {
        let cube = ModelCube {
            from: Vec3::new(0.0, 0.5, 0.0),
            to: Vec3::new(1.0, 0.5, 1.0),
            origin: Vec3::ZERO,
            rotation: Vec3::ZERO,
            faces: [Some([0.0, 0.0, 1.0, 1.0]); 6],
        };
        let support = ModelCube {
            from: Vec3::ZERO,
            to: Vec3::new(1.0, 0.5, 1.0),
            origin: Vec3::ZERO,
            rotation: Vec3::ZERO,
            faces: [Some([0.0, 0.0, 1.0, 1.0]); 6],
        };
        let all = [cube, support];
        assert_eq!(
            render_face_bias(&all[0], &all, Face::PosY),
            Some(Vec3::Y * FLAT_FACE_BIAS)
        );
        assert_eq!(render_face_bias(&all[0], &all, Face::NegY), None);
        assert_eq!(render_face_bias(&all[0], &all, Face::PosX), None);
        assert_eq!(render_face_bias(&all[0], &all, Face::PosZ), None);
    }

    #[test]
    fn flat_model_cubes_bias_away_from_backing_surface() {
        let poster = ModelCube {
            from: Vec3::new(0.0, 0.0, 0.5),
            to: Vec3::new(1.0, 1.0, 0.5),
            origin: Vec3::ZERO,
            rotation: Vec3::ZERO,
            faces: [Some([0.0, 0.0, 1.0, 1.0]); 6],
        };
        let backing = ModelCube {
            from: Vec3::new(0.0, 0.0, 0.5),
            to: Vec3::new(1.0, 1.0, 0.75),
            origin: Vec3::ZERO,
            rotation: Vec3::ZERO,
            faces: [Some([0.0, 0.0, 1.0, 1.0]); 6],
        };
        let all = [poster, backing];
        assert_eq!(
            render_face_bias(&all[0], &all, Face::NegZ),
            Some(Vec3::NEG_Z * FLAT_FACE_BIAS)
        );
        assert_eq!(render_face_bias(&all[0], &all, Face::PosZ), None);
    }

    #[test]
    fn unsupported_flat_model_cubes_fall_back_to_authored_positive_face() {
        let mut cube = ModelCube {
            from: Vec3::new(0.0, 0.5, 0.0),
            to: Vec3::new(1.0, 0.5, 1.0),
            origin: Vec3::ZERO,
            rotation: Vec3::ZERO,
            faces: [Some([0.0, 0.0, 1.0, 1.0]); 6],
        };
        let all = [cube.clone()];
        assert_eq!(
            render_face_bias(&cube, &all, Face::PosY),
            Some(Vec3::Y * FLAT_FACE_BIAS)
        );
        cube.faces[face_slot(Face::PosY)] = None;
        let all = [cube.clone()];
        assert_eq!(
            render_face_bias(&cube, &all, Face::NegY),
            Some(Vec3::NEG_Y * FLAT_FACE_BIAS)
        );
    }

    #[test]
    fn thick_model_cubes_emit_all_faces_without_bias() {
        let cube = ModelCube {
            from: Vec3::ZERO,
            to: Vec3::ONE,
            origin: Vec3::ZERO,
            rotation: Vec3::ZERO,
            faces: [Some([0.0, 0.0, 1.0, 1.0]); 6],
        };

        for face in Face::ALL {
            assert_eq!(
                render_face_bias(&cube, std::slice::from_ref(&cube), face),
                Some(Vec3::ZERO)
            );
        }
    }

    #[test]
    fn every_footprint_cell_is_covered_and_splits_the_cubes() {
        let inst = instance(WB);
        // Each cube is assigned to exactly one cell (the split partitions geometry).
        let total: usize = inst.cells.iter().map(|c| c.cubes.len()).sum();
        assert_eq!(
            total,
            inst.cubes.len(),
            "every cube assigned to exactly one cell"
        );
        // The lower cells (resting on the floor, full Z) are present and collide.
        for off in [[0, 0, 0], [1, 0, 0]] {
            let c = inst.cell(off).expect("floor cell present");
            assert!(!c.collision.is_empty(), "floor cell {off:?} collides");
        }
    }

    #[test]
    fn cells_are_local_and_within_unit_bounds() {
        let inst = instance(WB);
        for c in &inst.cells {
            for b in &c.collision {
                for i in 0..3 {
                    assert!(
                        b.min[i] >= -1e-3 && b.max[i] <= 1.0 + 1e-3,
                        "cell-local box"
                    );
                    assert!(b.max[i] > b.min[i]);
                }
            }
        }
    }

    #[test]
    fn footprint_geometry_fits_the_cell_box() {
        let inst = instance(WB);
        let (mn, mx) = (inst.bounds_min, inst.bounds_max);
        assert!(mn[0] >= -1e-3 && mx[0] <= 2.0 + 1e-3, "X within 2 cells");
        assert!(mn[1] >= -1e-3 && mx[1] <= 2.0 + 1e-3, "Y within 2 cells");
        assert!(mn[2] >= -1e-3 && mx[2] <= 1.0 + 1e-3, "Z within 1 cell");
    }

    #[test]
    fn collision_is_the_multi_box_model_shape_not_one_coarse_box() {
        // The fix: collision follows the actual cubes (several boxes per cell), so the
        // workbench isn't one solid 2×2×1 block. The bottom cells (legs + body + top) get
        // many boxes; the outline is the whole model's tight box across all cells.
        let inst = instance(WB);
        let floor = inst.cell([0, 0, 0]).expect("floor cell");
        assert!(
            floor.collision.len() > 1,
            "collision is multiple cube boxes, not one"
        );
        // Outline spans the whole 2×2×1 footprint (one box hugging the model).
        assert!(
            inst.bounds_max[0] - inst.bounds_min[0] > 1.5,
            "outline spans ~2 cells wide"
        );
        assert!(
            inst.bounds_max[1] - inst.bounds_min[1] > 1.0,
            "outline spans >1 cell tall"
        );
    }

    #[test]
    fn display_poses_are_parsed_and_cached() {
        // The workbench authors a full `display` block; the compile must capture the gui +
        // first-person poses (so the icon/held item pose as designed) rather than identity.
        let m = BlockModel::compile(&model_bytes(WB)).expect("compiles");
        let gui = m.display.gui;
        let fp = m.display.firstperson_righthand;
        // Non-identity rotations were authored for both contexts.
        assert_ne!(gui.rotation, [0.0; 3], "gui pose has an authored rotation");
        assert_ne!(
            fp.rotation, [0.0; 3],
            "first-person pose has an authored rotation"
        );
        // The cached accessor returns the same parsed data.
        assert_eq!(display(WB).gui, gui);
        // A finite pose matrix is produced for posing.
        assert!(fp
            .base_matrix()
            .to_cols_array()
            .iter()
            .all(|f| f.is_finite()));
    }

    /// The display euler must compose exactly as Blockbench/three.js 'XYZ' does
    /// (matrix `Rx·Ry·Rz`) — the convention the in-hand pose replication depends on.
    /// Single-axis mappings pin each axis's direction; the composed case pins the order.
    #[test]
    fn display_base_matrix_matches_blockbench_euler_convention() {
        let with_rot = |r: [f32; 3]| DisplayTransform {
            rotation: r,
            ..Default::default()
        };
        let close = |a: Vec3, b: Vec3| (a - b).length() < 1e-5;
        // Ry(+90°): +X → −Z (yaw left, as in Blockbench's preview).
        let m = with_rot([0.0, 90.0, 0.0]).base_matrix();
        assert!(close(m.transform_vector3(Vec3::X), -Vec3::Z));
        // Rx(+90°): +Y → +Z (pitch toward the viewer's side).
        let m = with_rot([90.0, 0.0, 0.0]).base_matrix();
        assert!(close(m.transform_vector3(Vec3::Y), Vec3::Z));
        // Order Rx·Ry: +X goes through Ry first (→ −Z), then Rx (→ +Y).
        let m = with_rot([90.0, 90.0, 0.0]).base_matrix();
        assert!(close(m.transform_vector3(Vec3::X), Vec3::Y));
    }

    /// With a `rotation_pivot` authored, the pose must rotate ABOUT that point: the
    /// pivot itself only moves by the authored translation. Pins the Blockbench
    /// position-correction algorithm (`pos -= R·piv − piv`).
    #[test]
    fn display_base_matrix_rotates_about_the_authored_pivot() {
        let piv = Vec3::new(0.25, -0.5, 0.125);
        let t = DisplayTransform {
            rotation: [16.0, 14.0, 4.0],
            translation: [1.0, 2.0, 3.0],
            rotation_pivot: piv.to_array(),
            ..Default::default()
        };
        let moved = t.base_matrix().transform_point3(piv);
        let expected = piv + Vec3::new(1.0, 2.0, 3.0) / 16.0;
        assert!(
            (moved - expected).length() < 1e-5,
            "pivot must stay fixed under rotation (moved to {moved:?}, expected {expected:?})"
        );
    }

    /// `display_from_unit` must be a POSITIVE uniform rescale + translation — no
    /// rotation, no mirrored axis. Any flip smuggled in here (the historical 180°-yaw /
    /// mirrored-euler hand bugs) would silently mis-pose every held model again.
    #[test]
    fn display_from_unit_is_an_unmirrored_uniform_rescale() {
        let m = instance(WB).display_from_unit;
        let (x, y, z) = (
            m.transform_vector3(Vec3::X),
            m.transform_vector3(Vec3::Y),
            m.transform_vector3(Vec3::Z),
        );
        for (v, axis) in [(x, Vec3::X), (y, Vec3::Y), (z, Vec3::Z)] {
            let k = v.dot(axis);
            assert!(k > 0.0, "axis {axis:?} must keep its direction, got {v:?}");
            assert!(
                (v - axis * k).length() < 1e-6,
                "axis {axis:?} must not rotate, got {v:?}"
            );
        }
        assert!(
            (x.length() - y.length()).abs() < 1e-6 && (y.length() - z.length()).abs() < 1e-6,
            "rescale must be uniform"
        );
    }
}
