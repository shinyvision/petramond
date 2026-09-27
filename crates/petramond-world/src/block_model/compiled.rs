use glam::{Mat4, Quat, Vec3};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::asset_cache::CompiledAsset;
use crate::bbmodel::{euler_quat, Model};
use crate::block::Aabb;

use super::defs::{part_role, PartRole};
use super::{all, def, posed_cube_bounds, BlockDisplay};

#[derive(Clone, Serialize, Deserialize)]
pub struct ModelCube {
    pub name: String,
    pub from: Vec3,
    pub to: Vec3,
    pub origin: Vec3,
    pub rotation: Vec3,
    pub faces: [Option<crate::bbmodel::FaceUv>; 6],
    pub cull: [Option<u8>; 6],
}

#[derive(Serialize, Deserialize)]
pub struct BlockModel {
    pub cubes: Vec<ModelCube>,
    pub texture_rgba: Vec<u8>,
    pub tex_w: u32,
    pub tex_h: u32,
    pub collision: Vec<Aabb>,
    /// Tight bounding box over all cubes, model space. Used for raycast/outline so the wireframe
    /// hugs the real extent. Cached from geometry.
    pub bounds: Aabb,
    /// The Blockbench `display` poses (hand / GUI / …), cached so the held item + slot
    /// icon orient the model exactly as authored rather than via a hardcoded angle.
    pub display: BlockDisplay,
    /// Pivot for the `display` poses, in authored pixel coords. It's the middle of the 16³ block
    /// cell, because that's where Blockbench pivots its previews no matter how big the model is.
    /// Centered formats (`bedrock_block`) put it at `(0, 8, 0)`, corner-grid ones (`java_block`)
    /// at `(8, 8, 8)`.
    pub display_pivot: [f32; 3],
}

impl BlockModel {
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

    fn parse_culls(root: &Value) -> Vec<[Option<u8>; 6]> {
        const DIRS: [(&str, u8); 6] = [
            ("east", 0),
            ("west", 1),
            ("up", 2),
            ("down", 3),
            ("south", 4),
            ("north", 5),
        ];
        let empty = Vec::new();
        let elements = root
            .get("elements")
            .and_then(Value::as_array)
            .unwrap_or(&empty);
        elements
            .iter()
            .filter(|e| e.get("type").and_then(Value::as_str).unwrap_or("cube") == "cube")
            .map(|e| {
                let mut cull = [None; 6];
                if let Some(faces) = e.get("faces") {
                    for (name, slot) in DIRS {
                        let dir = faces
                            .get(name)
                            .and_then(|f| f.get("cullface"))
                            .and_then(Value::as_str)
                            .and_then(|c| DIRS.iter().find(|(n, _)| *n == c))
                            .map(|(_, s)| *s);
                        cull[slot as usize] = dir;
                    }
                }
                cull
            })
            .collect()
    }

    /// Keep the cube geometry + texture from a parsed mob-frontend [`Model`] and BAKE
    /// the collision boxes + bounding box from that geometry. A block has no
    /// animations, but authored GROUP rotations are part of the rest pose Blockbench
    /// displays (the bed is authored under a 90°-turned group) — they are baked into
    /// each cube here (composed rotation + shifted box, an exact equivalence) so the
    /// compiled model matches the Blockbench scene.
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
                        cull: [None; 6],
                    };
                }
                let rot = Quat::from_mat4(&pose) * euler_quat(c.rotation);
                let (ez, ey, ex) = rot.to_euler(glam::EulerRot::ZYX);
                let origin = pose.transform_point3(c.origin);
                let shift = origin - c.origin;
                ModelCube {
                    name: c.name.clone(),
                    from: c.from + shift,
                    to: c.to + shift,
                    origin,
                    rotation: Vec3::new(ex.to_degrees(), ey.to_degrees(), ez.to_degrees()),
                    faces: c.faces,
                    cull: [None; 6],
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
            display: BlockDisplay::default(),
            display_pivot: [0.0, 8.0, 0.0],
        };
        model.rebake();
        model
    }

    pub(in crate::block_model) fn rebake(&mut self) {
        let (collision, bounds) = bake_collision_bounds(&self.cubes, |_| true);
        self.collision = collision;
        self.bounds = bounds;
    }

    pub(in crate::block_model) fn apply_part_roles(
        &mut self,
        roles: &[(&str, PartRole)],
        other: PartRole,
        row_key: &str,
    ) {
        for (name, _) in roles {
            if !self.cubes.iter().any(|c| c.name == *name) {
                log::warn!("block model '{row_key}': part '{name}' matches no cube");
            }
        }
        let role = |name: &str| part_role(roles, other, name);
        self.cubes.retain(|c| role(&c.name) != PartRole::Hidden);
        let (collision, bounds) = bake_collision_bounds(&self.cubes, |c| role(&c.name).collides());
        self.collision = collision;
        self.bounds = bounds;
    }

    fn offset_parts(&mut self, offsets: &[(&str, [f32; 3])], row_key: &str) {
        for (name, off) in offsets {
            let off = Vec3::from_array(*off);
            let mut hit = false;
            for c in self.cubes.iter_mut().filter(|c| c.name == *name) {
                c.from += off;
                c.to += off;
                c.origin += off;
                hit = true;
            }
            if !hit {
                log::warn!("block model '{row_key}': offset part '{name}' matches no cube");
            }
        }
    }
}

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
        if include_in_collision(c)
            && !super::geometry::cube_is_flat_plane(c)
            && (mx - mn).min_element() > 1e-4
        {
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
    const MAGIC: [u8; 8] = *b"LLBLK\0\0\0";
    const FORMAT_VERSION: u32 = 13;
    const SUBDIR: &'static str = "models";
    const EXTENSION: &'static str = "llblock";

    fn compile(source: &[u8]) -> Result<Self, String> {
        let src = std::str::from_utf8(source).map_err(|e| format!("bbmodel utf-8: {e}"))?;
        let mut model = BlockModel::from_model(&Model::load(src)?);
        let root: Value = serde_json::from_str(src).map_err(|e| format!("json: {e}"))?;
        model.display = BlockDisplay::parse(&root);
        let culls = Self::parse_culls(&root);
        debug_assert_eq!(culls.len(), model.cubes.len(), "cullface/cube zip drifted");
        for (cube, cull) in model.cubes.iter_mut().zip(culls) {
            cube.cull = cull;
        }
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

static MODELS: crate::content::Slot<Vec<BlockModel>> = crate::content::Slot::new(
    "compiled block models",
    &[crate::content::stage::MODELS],
    compile_models,
);

#[inline]
pub(super) fn models() -> &'static [BlockModel] {
    MODELS.current()
}

fn compile_models(_: &crate::content::ContentRegistry) -> Result<Vec<BlockModel>, String> {
    Ok(all()
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
            if !d.part_offsets.is_empty() {
                model.offset_parts(d.part_offsets, d.key);
            }
            model.apply_part_roles(d.part_roles, d.other_parts, d.key);
            for (list, what) in [(d.parts, "part"), (d.tint_parts, "tint part")] {
                for name in list {
                    if !model.cubes.iter().any(|c| c.name == *name) {
                        log::warn!("block model '{}': {what} '{name}' matches no cube", d.key);
                    }
                }
            }
            model
        })
        .collect())
}
