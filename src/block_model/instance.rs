use std::sync::LazyLock;

use glam::{Mat4, Vec3};

use crate::bbmodel::{euler_quat, face_corners};
use crate::block::Aabb;
use crate::facing::Facing;
use crate::mathh::IVec3;
use crate::mesh::face::Face;
use crate::mesh::SHADES;

use super::{
    all, atlas, cell_of, clip_to_cell, def, oriented_cell_instance, placement_transform_fp,
    posed_cube_bounds, render_face_bias, union_clip_to_cell, BlockModelKind, CollisionSpec,
    FitMode, ModelCube, MODELS,
};

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

        // --- Map the model into footprint space, per the row's fit mode. Uses
        // the BAKED posed bounds so the fit, the outline, and the collision all
        // agree on the model's extent. ---
        let (mn, mx) = (Vec3::from(m.bounds.min), Vec3::from(m.bounds.max));
        let fp = Vec3::new(
            footprint[0] as f32,
            footprint[1] as f32,
            footprint[2] as f32,
        );
        let (scale, lo, anchor) = match d.fit {
            // Fill: uniform scale (no stretch) so the largest axis spans the
            // cell box, X/Z centred, resting on the floor in Y.
            FitMode::Fill => {
                let extent = mx - mn;
                // World units per model unit: the tightest axis sets a uniform
                // scale so the model fills its largest footprint axis and
                // keeps its proportions.
                let per_unit = [extent.x / fp.x, extent.y / fp.y, extent.z / fp.z]
                    .into_iter()
                    .fold(f32::MIN_POSITIVE, f32::max);
                let scale = 1.0 / per_unit;
                // Centre on X/Z within the footprint; floor on Y.
                let span = extent * scale;
                (
                    scale,
                    Vec3::new((fp.x - span.x) * 0.5, 0.0, (fp.z - span.z) * 0.5),
                    mn,
                )
            }
            // Native: authored pixels ARE the footprint grid (16 px = 1 cell,
            // authored origin = footprint origin); out-of-box geometry
            // overhangs visually and is clipped out of collision/selection by
            // the ordinary per-cell clipping below.
            FitMode::Native => (1.0 / 16.0, Vec3::ZERO, Vec3::ZERO),
        };
        let to_fp = |v: Vec3| lo + (v - anchor) * scale;
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
                name: c.name.clone(),
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
        // the footprint mapping (`p_px = anchor + (p_fp − lo)/scale` — the inverse of
        // `to_fp`, any fit mode), then rebase on the pivot in blocks. Uniform scale +
        // translation, folded into one matrix.
        let display_from_unit = {
            let uspan = fp.max_element().max(1.0);
            let pivot = Vec3::from(m.display_pivot);
            let per_unit = 1.0 / scale;
            let k = uspan * per_unit / 16.0;
            let offset = (anchor + (fp * 0.5 - lo) * per_unit - pivot) / 16.0;
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
