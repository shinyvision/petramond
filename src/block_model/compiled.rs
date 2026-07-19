use std::sync::LazyLock;

use glam::{Mat4, Quat, Vec3};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::asset_cache::CompiledAsset;
use crate::bbmodel::{euler_quat, Model};
use crate::block::Aabb;

use super::{all, def, posed_cube_bounds, BlockDisplay};

/// One cube of a model: an axis-aligned box with a pivot + static rotation and a
/// per-face UV rect (in `Face::ALL` order; `None` = the face is omitted). The cached
/// [`BlockModel`] stores these in MODEL space; the runtime [`ModelInstance`] re-stores
/// them in footprint space with atlas-remapped UVs.
#[derive(Clone, Serialize, Deserialize)]
pub struct ModelCube {
    /// The authored Blockbench element name — preserved because names carry
    /// per-ROW meaning: a `models.json` row may list `hidden_parts` by name
    /// (the unlit oven hides its `fire` cube; see [`BlockModel::hide_parts`]).
    pub name: String,
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
                        name: c.name.clone(),
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
                    name: c.name.clone(),
                    from: c.from + shift,
                    to: c.to + shift,
                    origin,
                    rotation: Vec3::new(ex.to_degrees(), ey.to_degrees(), ez.to_degrees()),
                    faces: c.faces,
                }
            })
            .collect();
        let mut model = BlockModel {
            cubes,
            texture_rgba: m.texture_rgba.clone(),
            tex_w: m.tex_w,
            tex_h: m.tex_h,
            collision: Vec::new(),
            bounds: Aabb {
                min: [0.0; 3],
                max: [1.0; 3],
            },
            // `from_model` has only the parsed geometry; `compile` fills the display
            // poses + pivot from the raw JSON (the mob frontend drops them).
            display: BlockDisplay::default(),
            display_pivot: [0.0, 8.0, 0.0],
        };
        model.rebake();
        model
    }

    /// Re-bake collision + bounds from the current cubes — required after any
    /// geometry change (the initial bake, per-row part hiding/posing).
    pub(in crate::block_model) fn rebake(&mut self) {
        let (collision, bounds) = bake_collision_bounds(&self.cubes, |_| true);
        self.collision = collision;
        self.bounds = bounds;
    }

    /// Re-bake collision from cubes matching `include_in_collision`, while
    /// keeping the bounds over ALL cubes so selection/outlines still hug the
    /// full visible model.
    fn rebake_with_collision_filter(&mut self, include_in_collision: impl Fn(&ModelCube) -> bool) {
        let (collision, _) = bake_collision_bounds(&self.cubes, include_in_collision);
        self.collision = collision;
    }

    /// Drop the cubes named in `hidden` and re-bake collision + bounds from
    /// what remains — the per-ROW `hidden_parts` filter, applied AFTER the
    /// cache load (two `models.json` rows may share one authored file, e.g. a
    /// machine's lit/unlit variants toggling a `fire` cube; the compiled
    /// `.llblock` always holds the full model). A name matching no cube warns:
    /// a typo must not silently show the part.
    fn hide_parts(&mut self, hidden: &[&str], row_key: &str) {
        for h in hidden {
            if !self.cubes.iter().any(|c| c.name == *h) {
                log::warn!("block model '{row_key}': hidden part '{h}' matches no cube");
            }
        }
        self.cubes.retain(|c| !hidden.contains(&c.name.as_str()));
        self.rebake();
    }

    /// Exclude the cubes named in `hidden` from collision while keeping them
    /// visible and selectable — the per-ROW `collision_hidden_parts` filter,
    /// applied after the cache load. A name matching no visible cube warns,
    /// unless it was already removed by `hidden_parts`.
    pub(in crate::block_model) fn hide_collision_parts(
        &mut self,
        hidden: &[&str],
        already_hidden: &[&str],
        row_key: &str,
    ) {
        for h in hidden {
            if !already_hidden.contains(h) && !self.cubes.iter().any(|c| c.name == *h) {
                log::warn!(
                    "block model '{row_key}': collision-hidden part '{h}' matches no cube"
                );
            }
        }
        self.rebake_with_collision_filter(|c| !hidden.contains(&c.name.as_str()));
    }

    /// Translate the cubes named in `offsets` (authored pixels) and re-bake
    /// collision + bounds — the per-ROW `part_offsets` posing, applied after
    /// the cache load like [`hide_parts`](Self::hide_parts): rows sharing one
    /// authored file place a part differently per variant (the composter's
    /// fill surface rising with its stages). `origin` moves with the box so a
    /// rotated part keeps rotating about its own pivot. A name matching no
    /// cube warns (a typo must not silently leave the part unposed) — unless
    /// the same row's `hidden` filter already removed it: hide runs first and
    /// already validated the name, so offsetting a hidden part is a no-op,
    /// not a typo.
    fn offset_parts(&mut self, offsets: &[(&str, [f32; 3])], hidden: &[&str], row_key: &str) {
        for (name, off) in offsets {
            let off = Vec3::from_array(*off);
            let mut hit = false;
            for c in self.cubes.iter_mut().filter(|c| c.name == *name) {
                c.from += off;
                c.to += off;
                c.origin += off;
                hit = true;
            }
            if !hit && !hidden.contains(name) {
                log::warn!("block model '{row_key}': offset part '{name}' matches no cube");
            }
        }
        self.rebake();
    }
}

/// Collision = one posed AABB per SOLID cube matching `include_in_collision`
/// (skip flat/degenerate — a zero-extent cube is decoration, not a wall).
/// Bounds = the tight box over ALL cubes (a cube-less model degrades to the
/// unit cell).
fn bake_collision_bounds(
    cubes: &[ModelCube],
    include_in_collision: impl Fn(&ModelCube) -> bool,
) -> (Vec<Aabb>, Aabb) {
    let mut collision = Vec::new();
    let mut bmn = Vec3::splat(f32::INFINITY);
    let mut bmx = Vec3::splat(f32::NEG_INFINITY);
    for c in cubes {
        let (mn, mx) = posed_cube_bounds(c);
        bmn = bmn.min(mn);
        bmx = bmx.max(mx);
        if include_in_collision(c) && (mx - mn).min_element() > 1e-4 {
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
    (collision, bounds)
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
    /// v8: cubes carry their authored element NAME (per-row `hidden_parts`
    /// filtering needs it).
    const FORMAT_VERSION: u32 = 8;
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

/// Every kind's compiled [`BlockModel`], indexed by raw kind id — the cached parse,
/// precached once on first use (compiling each `.bbmodel` → `.llblock` on a miss, else
/// fast-loading the `.llblock`).
pub(super) static MODELS: LazyLock<Vec<BlockModel>> = LazyLock::new(|| {
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
            let mut model = crate::asset_cache::load_or_compile::<BlockModel>(d.key, &src)
                .unwrap_or_else(|e| {
                    log::error!("block model precache failed for {k:?}: {e}");
                    BlockModel::empty()
                });
            // The cache always holds the FULL model; the row's part filter,
            // collision filter, and part poses are applied on top so rows
            // sharing one file stay one cache entry each (the cache is keyed by
            // row key) with independent visibility/collision/pose.
            if !d.hidden_parts.is_empty() {
                model.hide_parts(d.hidden_parts, d.key);
            }
            if !d.collision_hidden_parts.is_empty() {
                model.hide_collision_parts(d.collision_hidden_parts, d.hidden_parts, d.key);
            }
            if !d.part_offsets.is_empty() {
                model.offset_parts(d.part_offsets, d.hidden_parts, d.key);
            }
            model
        })
        .collect()
});
