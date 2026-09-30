mod template;
use template::bake_cell_template;
pub use template::model_face_tris;

use glam::{Mat4, Vec3};

use crate::bbmodel::{euler_quat, face_corners};
use crate::block::Aabb;
use crate::facing::Facing;
use crate::shade::{ContactShadowVertex, FaceShading};
use petramond_math::face::Face;

use super::ao::{bake_contact_field, bake_face_ao, CONTACT_GRID};
use super::query::face_texel_opaque;
use super::{
    all, atlas, cell_of, clip_to_cell, cube_is_flat_plane, def, models, oriented_cell_instance,
    placement_transform_fp, posed_cube_bounds, render_face_bias, union_clip_to_cell,
    BlockModelKind, CollisionSpec, FitMode, ModelCube,
};

pub struct CellInstance {
    pub offset: [u8; 3],
    pub cubes: Vec<u32>,
    pub collision: Vec<Aabb>,
    pub selection_min: [f32; 3],
    pub selection_max: [f32; 3],
}

pub struct OrientedCellInstance {
    pub offset: [u8; 3],
    pub collision: Vec<Aabb>,
    pub selection_min: [f32; 3],
    pub selection_max: [f32; 3],
}

#[derive(Copy, Clone)]
pub struct ModelTemplateVertex {
    pub pos: Vec3,
    pub uv: [f32; 2],
    pub shade: f32,
    pub tinted: bool,
    pub appearance: super::FaceAppearance,
}

pub struct ModelCellTemplate {
    pub verts: Vec<ModelTemplateVertex>,
    pub indices: Vec<u32>,
    /// The geometry as contiguous runs, each with its emit-time gate (parts-mask
    /// bit, cullface neighbour test) and stream route (opaque-cutout vs
    /// alpha-blend). Baking them contiguously is what lets the mesher emit an
    /// arbitrary parts mask / neighbour configuration as a handful of slice
    /// copies instead of a per-cube filter.
    pub segments: Vec<TemplateSegment>,
}

#[derive(Copy, Clone, Debug)]
pub struct TemplateSegment {
    pub run: PartRun,
    pub blend: bool,
    pub part: Option<u8>,
    pub cull: Option<Face>,
}

#[derive(Copy, Clone, Debug, Default)]
pub struct PartRun {
    pub vert_start: u32,
    pub vert_len: u32,
    pub index_start: u32,
    pub index_len: u32,
}

/// One single-cell piece of a contact-shadow stamp: non-indexed triangles over
/// exactly ONE floor cell (the owner's own cell, `cell_delta == [0, 0]`, or a
/// ring cell of the footprint's one-cell dilation), positions relative to the
/// rotated footprint base (like [`ModelCellTemplate`] vertices), `darken` the
/// multiplicative strength. `cell_delta` is the stamped cell's world `(dx, dz)`
/// from the OWNER cell, already rotated by the facing — the mesher gates each
/// piece on ITS OWN cell's support (opaque full cube below, not buried), which
/// is what lets the stamp spill onto the grass next to the model while a
/// missing neighbour floor still clips it per cell.
pub struct ContactPiece {
    pub cell_delta: [i32; 2],
    pub verts: Vec<ContactShadowVertex>,
}

/// The startup-baked contact-shadow stamp owned by ONE bottom footprint cell at
/// one facing. Ring cells OUTSIDE the footprint are assigned to exactly one
/// adjacent bottom cell (nearest, index tie-break) so two cells of one model
/// can never stamp the same world cell — the multiplicative pass would darken
/// it twice. Empty for non-bottom cells and cells whose fields baked to
/// nothing.
pub struct ContactCellTemplate {
    pub pieces: Vec<ContactPiece>,
}

pub struct ModelInstance {
    pub footprint: [u8; 3],
    pub cubes: Vec<ModelCube>,
    pub cells: Vec<CellInstance>,
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    pub cube_boxes: Vec<Aabb>,
    pub oriented_cells: [Vec<OrientedCellInstance>; 4],
    /// Per-facing, per-cell baked render geometry — the chunk-mesher's hot path. Indexed
    /// by [`Facing::to_u8`] then by the SAME order as [`Self::cells`] (use
    /// [`Self::cell_template`]). All the static work (cube rotation, placement rotation,
    /// face bias, degenerate-face culling, atlas UVs, directional shade) is resolved here
    /// once so a remesh just translates + lights the verts.
    pub oriented_render: [Vec<ModelCellTemplate>; 4],
    pub face_ao: Vec<[[f32; 4]; 6]>,
    /// Per-cube, per-face draw flag: false when the face's atlas rect is FULLY    /// transparent. Such a face discards every fragment at mip 0, so it is
    /// dropped from every bake — otherwise the cutout mip chain promotes its
    /// texels to opaque with the neighbouring artwork's colour and an invisible
    /// sliver face renders as a bright line at distance. Consulted by the
    /// held/dropped/icon bakes too, so every presentation drops the same faces.
    pub face_draw: Vec<[bool; 6]>,
    pub oriented_contact: [Vec<ContactCellTemplate>; 4],
    pub display_from_unit: Mat4,
}

impl ModelInstance {
    #[inline]
    pub fn cell(&self, offset: [u8; 3]) -> Option<&CellInstance> {
        self.cells.iter().find(|c| c.offset == offset)
    }

    #[inline]
    pub fn oriented_cell(&self, offset: [u8; 3], facing: Facing) -> Option<&OrientedCellInstance> {
        self.oriented_cells[facing.to_u8() as usize]
            .iter()
            .find(|c| c.offset == offset)
    }

    #[inline]
    pub fn cell_template(&self, offset: [u8; 3], facing: Facing) -> Option<&ModelCellTemplate> {
        let idx = self.cells.iter().position(|c| c.offset == offset)?;
        Some(&self.oriented_render[facing.to_u8() as usize][idx])
    }

    #[inline]
    pub fn contact_template(
        &self,
        offset: [u8; 3],
        facing: Facing,
    ) -> Option<&ContactCellTemplate> {
        let idx = self.cells.iter().position(|c| c.offset == offset)?;
        let tmpl = &self.oriented_contact[facing.to_u8() as usize][idx];
        (!tmpl.pieces.is_empty()).then_some(tmpl)
    }

    fn build(kind: BlockModelKind) -> Self {
        let m = &models()[kind.0 as usize];
        let d = def(kind);
        let footprint = d.cells.map(|c| c.max(1));
        let at = atlas();

        let (mn, mx) = (Vec3::from(m.bounds.min), Vec3::from(m.bounds.max));
        let fp = Vec3::new(
            footprint[0] as f32,
            footprint[1] as f32,
            footprint[2] as f32,
        );
        let (scale, lo, anchor) = match d.fit {
            FitMode::Fill => {
                let extent = mx - mn;
                let per_unit = [extent.x / fp.x, extent.y / fp.y, extent.z / fp.z]
                    .into_iter()
                    .fold(f32::MIN_POSITIVE, f32::max);
                let scale = 1.0 / per_unit;
                let span = extent * scale;
                (
                    scale,
                    Vec3::new((fp.x - span.x) * 0.5, 0.0, (fp.z - span.z) * 0.5),
                    mn,
                )
            }
            FitMode::Native => (1.0 / 16.0, Vec3::ZERO, Vec3::ZERO),
            FitMode::Centered => (
                1.0 / 16.0,
                Vec3::new(fp.x * 0.5, fp.y - mx.y / 16.0, fp.z * 0.5),
                Vec3::ZERO,
            ),
        };
        let to_fp = |v: Vec3| lo + (v - anchor) * scale;
        let to_fp_box = |b: &Aabb| Aabb {
            min: to_fp(Vec3::from(b.min)).to_array(),
            max: to_fp(Vec3::from(b.max)).to_array(),
        };

        let cubes: Vec<ModelCube> = m
            .cubes
            .iter()
            .map(|c| ModelCube {
                name: c.name.clone(),
                from: to_fp(c.from),
                to: to_fp(c.to),
                origin: to_fp(c.origin),
                rotation: c.rotation,
                cull: c.cull,
                faces: c.faces.map(|f| {
                    f.map(|face| {
                        let [u0, v0, u1, v1] = face.uv;
                        let [au0, av0] = at.remap(kind, [u0, v0]);
                        let [au1, av1] = at.remap(kind, [u1, v1]);
                        face.with_uv([au0, av0, au1, av1])
                    })
                }),
            })
            .collect();

        let footprint_collision: Vec<Aabb> = match d.collision {
            CollisionSpec::FromModel => m.collision.iter().map(&to_fp_box).collect(),
        };
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

        let mut cells = Vec::new();
        for dz in 0..footprint[2] {
            for dy in 0..footprint[1] {
                for dx in 0..footprint[0] {
                    let offset = [dx, dy, dz];
                    let o = Vec3::new(dx as f32, dy as f32, dz as f32);
                    let cube_idx: Vec<u32> = cubes
                        .iter()
                        .enumerate()
                        .filter(|(_, c)| cell_of((c.from + c.to) * 0.5, footprint) == offset)
                        .map(|(i, _)| i as u32)
                        .collect();
                    let collision: Vec<Aabb> = footprint_collision
                        .iter()
                        .filter_map(|b| clip_to_cell(b, o))
                        .collect();
                    let sel = union_clip_to_cell(&cube_boxes, o);
                    let (selection_min, selection_max) = match sel {
                        Some(s) => (s.min, s.max),
                        None => ([0.0; 3], [0.0; 3]),
                    };
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

        // Self-AO: per-face corner shade multipliers from the fitted cubes, with
        // the production alpha test (a cutout texel does not cast). Facing-
        // independent — occlusion is intrinsic to the geometry — so one bake
        // serves all four oriented templates plus the item/icon bakes.
        // A cube whose role does not draw (a hitbox) is transparent to every
        // occlusion and draw question: it casts no self-AO, stamps no contact
        // shadow, and emits no face.
        let draws = |cube: &ModelCube| d.part_role(&cube.name).draws();
        let face_ao = bake_face_ao(&cubes, |cube, face, mn, mx, hit| {
            draws(cube) && face_texel_opaque(cube, face, mn, mx, hit, at)
        });
        let contact_casters: Vec<ModelCube> = cubes.iter().filter(|c| draws(c)).cloned().collect();
        // Per-face alpha classification of the atlas rect: `face_blend` picks the
        // stream route (any partial-alpha texel 1..=254 → alpha-blended instead of
        // opaque-cutout); `face_draw` drops faces whose rect is FULLY transparent —
        // they discard every fragment at mip 0 anyway, and keeping them lets the
        // cutout mip chain promote their texels to opaque with the neighbouring
        // artwork's colour (a 1px panel's invisible side sliver rendering as a
        // bright line — the forge furnace coals).
        let mut face_draw: Vec<[bool; 6]> = Vec::with_capacity(cubes.len());
        let mut face_blend: Vec<[bool; 6]> = Vec::with_capacity(cubes.len());
        for c in &cubes {
            let mut draw = [false; 6];
            let mut blend = [false; 6];
            for (slot, f) in c.faces.iter().enumerate() {
                if let Some(uv) = f {
                    let (visible, b) = at.rect_alpha_class(uv.uv);
                    draw[slot] = visible && draws(c);
                    blend[slot] = b;
                }
            }
            face_draw.push(draw);
            face_blend.push(blend);
        }
        // Contact-shadow fields, authored space: every bottom footprint cell's
        // own floor, PLUS the one-cell dilation ring around the footprint so
        // the stamp can spill onto neighbouring terrain instead of shearing
        // off at the cell boundary. Each ring cell is assigned to exactly one
        // adjacent bottom cell (nearest by chebyshev, index tie-break): unique
        // ownership, because two owners stamping one world cell would darken
        // it twice under the multiplicative pass. (Ring cells whose only
        // adjacent footprint cells were dropped as empty find no owner and
        // are skipped — the mesher's ±1-cell pad couldn't gate them anyway.)
        struct AuthoredContact {
            owner: usize,
            cell: [i32; 2],
            delta: [i32; 2],
            field: [[f32; CONTACT_GRID]; CONTACT_GRID],
        }
        let bottoms: Vec<usize> = cells
            .iter()
            .enumerate()
            .filter(|(_, c)| c.offset[1] == 0)
            .map(|(i, _)| i)
            .collect();
        let mut authored_contact: Vec<AuthoredContact> = Vec::new();
        for &bi in &bottoms {
            let o = cells[bi].offset;
            let cell = [o[0] as i32, o[2] as i32];
            if let Some(field) = bake_contact_field(&contact_casters, cell[0], cell[1]) {
                authored_contact.push(AuthoredContact {
                    owner: bi,
                    cell,
                    delta: [0, 0],
                    field,
                });
            }
        }
        for cx in -1..=footprint[0] as i32 {
            for cz in -1..=footprint[2] as i32 {
                let inside = (0..footprint[0] as i32).contains(&cx)
                    && (0..footprint[2] as i32).contains(&cz);
                if inside {
                    continue;
                }
                let Some(field) = bake_contact_field(&contact_casters, cx, cz) else {
                    continue;
                };
                let owner = bottoms
                    .iter()
                    .map(|&bi| {
                        let o = cells[bi].offset;
                        let d = (cx - o[0] as i32).abs().max((cz - o[2] as i32).abs());
                        (d, bi)
                    })
                    .filter(|&(d, _)| d <= 1)
                    .min();
                let Some((_, bi)) = owner else { continue };
                let o = cells[bi].offset;
                authored_contact.push(AuthoredContact {
                    owner: bi,
                    cell: [cx, cz],
                    delta: [cx - o[0] as i32, cz - o[2] as i32],
                    field,
                });
            }
        }

        // Bake the per-facing render geometry once. `placement_transform` gives the
        // facing's rotation + footprint shift relative to the base; the mesher adds the
        // integer world base at remesh. Per-cube and per-face math (quaternions,
        // matrix products, face bias, degenerate-face culling) is
        // resolved here.
        let oriented_render = std::array::from_fn(|i| {
            let facing = Facing::from_u8(i as u8);
            let base_xform = placement_transform_fp(footprint, facing);
            cells
                .iter()
                .map(|cell| {
                    bake_cell_template(
                        base_xform,
                        &cubes,
                        &cell.cubes,
                        &face_ao,
                        &face_draw,
                        &face_blend,
                        d.parts,
                        d.tint_parts,
                        |uv| atlas().appearance(kind, uv),
                    )
                })
                .collect()
        });
        let oriented_contact = std::array::from_fn(|i| {
            let facing = Facing::from_u8(i as u8);
            let base_xform = placement_transform_fp(footprint, facing);
            (0..cells.len())
                .map(|ci| ContactCellTemplate {
                    pieces: authored_contact
                        .iter()
                        .filter(|a| a.owner == ci)
                        .filter_map(|a| {
                            let rd = base_xform.transform_vector3(Vec3::new(
                                a.delta[0] as f32,
                                0.0,
                                a.delta[1] as f32,
                            ));
                            let piece = ContactPiece {
                                cell_delta: [rd.x.round() as i32, rd.z.round() as i32],
                                verts: bake_contact_piece(base_xform, a.cell, &a.field),
                            };
                            (!piece.verts.is_empty()).then_some(piece)
                        })
                        .collect(),
                })
                .collect()
        });

        // Centred-unit item space to authored display space, blocks around the display pivot:
        // undo the bake centring, undo the footprint mapping (`to_fp`, any fit mode), rebase on
        // the pivot. Scale plus translate, so it all folds into one matrix.
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
            face_ao,
            face_draw,
            oriented_contact,
            display_from_unit,
        }
    }
}

/// Bakes one floor cell's contact field (own cell or dilation ring, authored
/// coords) into rotated, base-relative triangles. Stamp is sparse. Quads with
/// all corners rounding to zero get dropped, so only shadowed floor makes it in.
fn bake_contact_piece(
    base_xform: Mat4,
    cell: [i32; 2],
    field: &[[f32; CONTACT_GRID]; CONTACT_GRID],
) -> Vec<ContactShadowVertex> {
    const SKIP_EPS: f32 = 1e-3;
    let mut verts = Vec::new();
    let step = 1.0 / (CONTACT_GRID - 1) as f32;
    let corner = |i: usize, j: usize| {
        let p = base_xform.transform_point3(Vec3::new(
            cell[0] as f32 + i as f32 * step,
            0.0,
            cell[1] as f32 + j as f32 * step,
        ));
        ContactShadowVertex {
            pos: p.to_array(),
            darken: field[i][j],
        }
    };
    for i in 0..CONTACT_GRID - 1 {
        for j in 0..CONTACT_GRID - 1 {
            let quad = [
                corner(i, j),
                corner(i + 1, j),
                corner(i + 1, j + 1),
                corner(i, j + 1),
            ];
            if quad.iter().all(|v| v.darken < SKIP_EPS) {
                continue;
            }
            verts.extend_from_slice(&[quad[0], quad[1], quad[2], quad[0], quad[2], quad[3]]);
        }
    }
    verts
}

static INSTANCES: crate::content::Slot<Vec<ModelInstance>> = crate::content::Slot::new(
    "block model instances",
    &[crate::content::stage::MODELS],
    build_instances,
);

fn build_instances(
    registry: &crate::content::ContentRegistry,
) -> Result<Vec<ModelInstance>, String> {
    use rayon::prelude::*;
    let content = crate::content::Content::current();
    if !std::ptr::eq(content.registry(), registry) {
        return Ok(all().iter().map(|&k| ModelInstance::build(k)).collect());
    }
    Ok(all()
        .par_iter()
        .map(|&k| {
            let _pin = crate::content::pin(content);
            ModelInstance::build(k)
        })
        .collect())
}

#[inline]
pub fn instance(kind: BlockModelKind) -> &'static ModelInstance {
    &INSTANCES.current()[kind.0 as usize]
}

#[inline]
pub fn footprint(kind: BlockModelKind) -> [u8; 3] {
    instance(kind).footprint
}

#[cfg(test)]
mod tests;
