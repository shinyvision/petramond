use glam::IVec3;

use crate::atlas::Tile;
use crate::block::CellView;
use crate::block::{Block, ShapeFamily};
use crate::block_state::{LogAxis, SlabState};
use crate::chunk::{section_idx, SectionPos, SECTION_SIZE, SECTION_VOLUME, SKY_FULL};
use crate::section::Section;

use super::super::face::{quad_for, Face, FACES};
use super::super::face_emit::{cube_face_lighting_pad, fold_light, push_cube_face_with_cell_uvs};
use super::super::greedy::{emit_greedy_quads, FlatFace, GreedyScratch, GREEDY};
use super::super::tint;
use super::super::vertex::{ChunkMesh, ModelVertex, UV_MODE_NONE};
use super::super::water::{self, SideVsWater, WaterSurface};

use super::super::boxset::{cell_seals_face, emit_box_set, BoxSetScratch, ShapeBox};
use super::cell_class::{cell_classes, BOXES, CROP, CROSS, FAST_CUBE, MODEL, SKIP, TORCH, WATER};
use super::cube_face::{
    boundary_plane, cube_face_lighting, cube_face_tile, face_axes, face_index, facing_face,
    log_side_cell_uvs, log_side_uvs_apply,
};
use super::exposed_masks::{build_exposed_masks, mask_has, VISIT_ALL};
use super::model_block::{emit_model_block, emit_model_contact};
use super::pad::{mesh_pad_idx, SectionMeshPad};
use super::plant::emit_plant;
use super::{LeafMeshMode, MeshOptions};

#[allow(clippy::too_many_arguments)]
pub(super) fn section_geometry(
    section: &Section,
    pos: SectionPos,
    neighbour_block: impl Fn(i32, i32, i32) -> u8,
    neighbour_cell_state: impl Fn(i32, i32, i32) -> crate::block::ShapeState,
    neighbour_water: impl Fn(i32, i32, i32) -> u8,
    neighbour_light: impl Fn(i32, i32, i32) -> u8,
    neighbour_blocklight: impl Fn(i32, i32, i32) -> crate::light::LightRgb,
    neighbour_loaded: impl Fn(i32, i32, i32) -> bool,
    tints: Option<&tint::BiomeTints>,
    options: MeshOptions,
    pad: Option<&SectionMeshPad<'_>>,
) -> ChunkMesh {
    let mut opaque = vec![];
    let mut transparent = vec![];
    let mut transparent_two_sided = vec![];
    let mut translucent = vec![];
    let mut model: Vec<ModelVertex> = vec![];
    let mut model_idx: Vec<u32> = vec![];
    let mut contact: Vec<super::super::vertex::ContactShadowVertex> = vec![];

    let (ox, oy, oz) = pos.origin_world();
    let tint_tile = |kind, ci| tints.map_or(tint::NO_TINT, |t| t.tile(kind, ci));
    let tint_water = |ci| tints.map_or(tint::NO_TINT, |t| t.water[ci]);
    // Per-cell `petramond:tint` presentation entries (replicated cell KV):
    // a multiply into the vertex tint lane. Sparse — empty on almost every
    // section, so the fast path is one `is_empty` test.
    let cell_tints = section.cell_tint_map();
    // One cell's tint for one of its parts. A single-part cell (every cube,
    // stair, chair — anything but a stacked slab) carries only part 0, so the
    // scan ends on the first entry.
    let part_tint = |cell: usize, part: crate::block::CellPart| -> Option<[f32; 3]> {
        if cell_tints.is_empty() {
            return None;
        }
        cell_tints
            .get(&(cell as u16))?
            .iter()
            .find(|&&(p, _)| p == part)
            .map(|&(_, m)| m)
    };
    // The cube path is whole-cell by nature: it draws part 0's tint.
    let kv_tint = |cell: usize, tint: [f32; 3]| -> [f32; 3] {
        match part_tint(cell, 0) {
            Some(m) => [tint[0] * m[0], tint[1] * m[1], tint[2] * m[2]],
            None => tint,
        }
    };
    // Whether the cell carries a tint at all — tinted cells set the vertex
    // dyed flag so faces sample their tiles' dye-base twins and the multiply
    // lands on a desaturated, peak-white base.
    let cell_tinted = |cell: usize| part_tint(cell, 0).is_some();
    // The ONE place a cell's `petramond:tint` reaches box geometry: multiply
    // it into every emitted face AND mark the box dyed (so the multiply lands
    // on the tile's dye-base twin). Every box family gets both halves by
    // construction. Threading the multiply through each family's own tint
    // closure instead left four of six families flagging the dye base without
    // ever multiplying, which rendered them whitened and untinted.
    //
    // Each box takes ITS OWN part's tint, so one cell can hold a dyed layer
    // and a plain one (a white slab under an orange one) and each draws right.
    let apply_cell_tint = |boxes: &mut Vec<ShapeBox>, cell: usize| {
        if cell_tints.is_empty() {
            return;
        }
        for b in boxes.iter_mut() {
            if let Some(m) = part_tint(cell, b.part) {
                b.apply_tint(m);
            }
        }
    };

    // Every block read is by world coord through the routing closure (in-section
    // and cross-section alike); out-of-world / unloaded reads return air.
    let block_at =
        |wx: i32, wy: i32, wz: i32| -> Block { Block::from_id(neighbour_block(wx, wy, wz)) };
    let slab_at = |wx: i32, wy: i32, wz: i32| -> Option<SlabState> {
        let block = block_at(wx, wy, wz);
        crate::slab::is_slab(block).then(|| {
            crate::slab::normalize_state(
                block,
                SlabState::from_cell(neighbour_cell_state(wx, wy, wz)),
            )
        })
    };
    // "Cell holds a full slab stack" — callers gate on `is_slab` first (dense flag)
    // so this only pays a state lookup on actual slab cells. Full stacks cull and
    // occlude AO/light like opaque cubes; no normalize needed (a normalized default
    // is a single layer, never full).
    let slab_full_at = |wx: i32, wy: i32, wz: i32| -> bool {
        SlabState::from_cell(neighbour_cell_state(wx, wy, wz)).is_full()
    };
    let water_at = |wx: i32, wy: i32, wz: i32| -> u8 { neighbour_water(wx, wy, wz) };
    let fluid_at = |wx: i32, wy: i32, wz: i32| -> Option<f32> {
        if block_at(wx, wy, wz) != Block::Water {
            return None;
        }
        Some(crate::world::water::fluid_height(
            water_at(wx, wy, wz),
            block_at(wx, wy + 1, wz),
        ))
    };
    let water_fills_cell = |wx: i32, wy: i32, wz: i32| -> bool {
        if block_at(wx, wy, wz) != Block::Water {
            return false;
        }
        crate::world::water::fills_cell(water_at(wx, wy, wz), block_at(wx, wy + 1, wz))
    };
    // Still-source probe for the flow gradient: two adjacent still sources
    // never flow into each other (see `water::surface_flow_dir`).
    let water_still_at = |wx: i32, wy: i32, wz: i32| -> bool {
        block_at(wx, wy, wz) == Block::Water
            && crate::world::water::is_still_source(water_at(wx, wy, wz))
    };

    // The primitive seam every shape family reads the world through.
    let nbh = MeshNeighborhood {
        block: &neighbour_block,
        cell_state: &neighbour_cell_state,
        section,
        origin: (ox, oy, oz),
    };

    // The unified box-set emitter's scratch + box buffer (see mesh::boxset):
    // every axis-aligned sub-cell shape family routes through it.
    let mut box_scratch = BoxSetScratch::default();
    let mut mesh_boxes: Vec<ShapeBox> = Vec::new();

    // The cell-local occupancy boxes of the block at `p` — what the box-set
    // emitter subtracts from a flush face so sub-cell geometry culls against
    // sub-cell geometry (a fence cap on a slab, a chain continuing into the
    // chain above). Whole opaque cells are handled by the cheaper solid cull
    // and contribute nothing here; families with no box form (plants, torch,
    // models, custom bakes across the section boundary) stay empty, which
    // just means "no sub-cell cull", never a wrong cull.
    let occ_scratch = std::cell::RefCell::new(Vec::<ShapeBox>::new());
    let occupancy_boxes = |p: IVec3, cell_block: Block, out: &mut Vec<([f32; 3], [f32; 3])>| {
        let nb_block = block_at(p.x, p.y, p.z);
        // Dense flag first: this runs per face of every box-shaped cell, and
        // the shape-kind row behind `resolves_to_boxes` is a big-table load
        // that almost every neighbour is rejected without needing.
        if !nb_block.has_box_shape() {
            return;
        }
        // See-through texels cannot seal ANOTHER block's face: a cutout
        // ladder panel flush on a stair would cull the stair's side and show
        // a hole through the rungs. Same-block contact still culls — the
        // glass/translucent convention the cube path uses — so stacked
        // panes/chains keep their exact box-vs-box culls.
        if (nb_block.is_transparent() || nb_block.is_translucent()) && nb_block != cell_block {
            return;
        }
        // The neighbour's own resolved boxes — the SAME producer the mesh
        // uses, so what culls a face is exactly what would have been drawn
        // there. Presentation is irrelevant to an occupancy query, so the
        // tint is a constant; the scratch keeps the per-face call allocation
        // free.
        let k = nb_block.shape_kind_def();
        let mut boxes = occ_scratch.borrow_mut();
        boxes.clear();
        let tint_for = |_: Tile| [1.0f32; 3];
        k.render.boxes(
            &crate::block::ShapeCtx {
                nb: &nbh,
                pos: p,
                block: nb_block,
                params: &k.params,
                tint_for: &tint_for,
                part_tint: crate::block::NO_PART_TINT,
            },
            &mut boxes,
        );
        out.extend(
            boxes
                .iter()
                .filter(|b| b.occludes)
                .map(|b| (b.aabb.min, b.aabb.max)),
        );
    };

    // "Does the cell above seal my top face?" — the cube path's only
    // sub-cell neighbour cull, asked of the NEIGHBOUR's own resolved boxes
    // (`boxset::cell_seals_face`), so it names no family and a mod shape with
    // a floor-flush base gets it for free. A sealed top face is invisible, and
    // a nearly-coplanar one is worse than invisible: a snow layer's top sits
    // 1/16 above its carrier's and the two z-fight from far above.
    // Deliberately PosY-only — a sealed face in the other five directions is
    // plain overdraw, never a visible artifact.
    let seal_scratch = std::cell::RefCell::new((Vec::<ShapeBox>::new(), BoxSetScratch::default()));
    let seals_floor = |p: IVec3| -> bool {
        let (boxes, scratch) = &mut *seal_scratch.borrow_mut();
        cell_seals_face(&nbh, p, Face::NegY, boxes, scratch)
    };

    // The shared sub-cell AO occupancy query: does the cell hold solid matter
    // overlapping the cell-local pocket AABB? Whole cell for opaque cubes /
    // full stacks; half-cell REFINED state for partial slabs and stairs (the
    // pad captures the stored refined bytes verbatim, so a stair's corner
    // byte is a free decode here — occupancy must track the drawn shape, not
    // the placed facing); the mask-free post for fences/panes (rails are thin
    // and corner-distant anyway); the panel for ladders; the render-bake
    // boxes for IN-SECTION custom cells (both meshers share `section`, so the
    // restriction is parity-safe). Consumed by the cube gathers' cast probes
    // AND the box emitter's out-of-cell probes, so casting and receiving are
    // one rule.
    let cell_matter = |cl: (i32, i32, i32), lo: [f32; 3], hi: [f32; 3]| -> bool {
        let (cx, cy, cz) = cl;
        let b = block_at(cx, cy, cz);
        if b.occludes_ao() {
            return true;
        }
        // Everything below the whole-cell case is the FAMILY's answer: each
        // one knows its own shape and its own parity constraints (see
        // `ShapeSim::occupies_pocket`). The mesher only asks — and asks the
        // same oracle the light flood's apertures come from.
        let k = b.shape_kind_def();
        k.sim
            .occupies_pocket(&k.params, &nbh, IVec3::new(cx, cy, cz), b, lo, hi)
    };

    // Reused per-thread greedy scratch: flat opaque cube faces are deferred here during the
    // cell scan, then merged into tiled quads after it. Taken out + put back so meshing
    // allocates nothing.
    let mut greedy = GREEDY.with(|g| {
        g.replace(GreedyScratch {
            faces: Vec::new(),
            merged: Vec::new(),
            gen: 0,
            slice_counts: [0; FACES.len() * SECTION_SIZE],
        })
    });
    let greedy_gen = greedy.begin();
    let exposed_masks = pad
        .filter(|_| options.leaf_mesh_mode == LeafMeshMode::Detailed)
        .map(|pad| build_exposed_masks(pad, (ox, oy, oz), &seals_floor));

    let classes = cell_classes();
    // Which cells in each `(ly, lz)` row have any work at all. With exposure
    // masks this excludes buried cubes outright, so a solid underground row
    // costs one word test instead of sixteen classified cells.
    let visit = exposed_masks
        .as_ref()
        .map_or(&VISIT_ALL, |m| m.visit_rows());
    for ly in 0..SECTION_SIZE {
        for lz in 0..SECTION_SIZE {
            let mut row = visit[ly * SECTION_SIZE + lz];
            while row != 0 {
                let lx = row.trailing_zeros() as usize;
                row &= row - 1;
                let id = section.block_raw(lx, ly, lz);
                // ONE dense byte answers every dispatch question below (see
                // `cell_class`). Air, chests and doors emit nothing here.
                let class = classes[id as usize];
                if class & SKIP != 0 {
                    continue;
                }
                let block = Block::from_id(id);

                let wx = ox + lx as i32;
                let wy = oy + ly as i32;
                let wz = oz + lz as i32;
                let ci = lz * SECTION_SIZE + lx;

                // The box-set emitter's world hooks for this cell (zero-cost
                // closures over the shared reads; only box-family cells call
                // them).
                let neighbor_solid = |face: Face| {
                    let (dx, dy, dz) = face.dir();
                    let nb = block_at(wx + dx, wy + dy, wz + dz);
                    nb.is_opaque() || (nb.is_slab() && slab_full_at(wx + dx, wy + dy, wz + dz))
                };
                let neighbor_boxes = |face: Face, out: &mut Vec<([f32; 3], [f32; 3])>| {
                    let (dx, dy, dz) = face.dir();
                    occupancy_boxes(IVec3::new(wx + dx, wy + dy, wz + dz), block, out);
                };

                if class & (CROSS | CROP) != 0 {
                    let shape = block.shape_family();
                    let tile = block.tiles()[0];
                    let l = neighbour_light(wx, wy, wz) as u32;
                    let bl = neighbour_blocklight(wx, wy, wz).channels().map(u32::from);
                    let (sky6, blight) = fold_light(l, bl, SKY_FULL as u32);
                    let tint = tint_tile(tile.world_tint(), ci);
                    // Layer-2 dimensions (a mod's retuned cross/crop) or the
                    // engine defaults for a parameterless row.
                    let dims = block.shape_kind_def().params.dimensions();
                    let (inset, drop) = if class & CROP != 0 {
                        (
                            dims.map_or(crate::block::CROP_PLANE_INSET, |d| d.inset),
                            dims.map_or(crate::block::CROP_PLANE_DROP, |d| d.drop),
                        )
                    } else {
                        (dims.map_or(0.0, |d| d.inset), 0.0)
                    };
                    emit_plant(
                        &mut opaque,
                        shape,
                        wx as f32,
                        wy as f32,
                        wz as f32,
                        tile,
                        tint,
                        sky6,
                        blight,
                        inset,
                        drop,
                    );
                    continue;
                }

                if class & TORCH != 0 {
                    let [top_tile, _bottom, side_tile] = block.tiles();
                    // Sky channel = the cell's skylight; block channel = the torch's own
                    // emission (self-lit). `max(sky_term, block_term)` in the shader
                    // equals the old single-channel `max(cell_sky, emission)` fold at
                    // identity scale, and the emission channel never dims at night.
                    let cell_sky = neighbour_light(wx, wy, wz) as u32;
                    let sky6 = ((cell_sky * 63 + SKY_FULL as u32 / 2) / SKY_FULL as u32).min(63);
                    let [er, eg, eb] = block.light_emission_rgb();
                    let emit =
                        crate::light::BlockLight6::from_x2(crate::light::LightRgb::new(er, eg, eb));
                    let placement = section.torch_placement(lx, ly, lz);
                    super::torch::emit_torch(
                        &mut opaque,
                        wx as f32,
                        wy as f32,
                        wz as f32,
                        placement,
                        side_tile,
                        top_tile,
                        [1.0, 1.0, 1.0],
                        sky6,
                        emit,
                    );
                    continue;
                }

                // Every box-shaped family resolves through its own facet: ONE
                // producer, so the drawn boxes are the boxes collision and
                // targeting read. Adding a family means implementing
                // `ShapeRender::boxes`, not editing the mesher.
                let mut slab_as_cube = false;
                if class & BOXES != 0 {
                    let kind = block.shape_kind_def();
                    let tint_for = |tile: Tile| tint_tile(tile.world_tint(), ci);
                    let cell_part_tint = |part| part_tint(section_idx(lx, ly, lz), part);
                    let ctx = crate::block::ShapeCtx {
                        nb: &nbh,
                        pos: IVec3::new(wx, wy, wz),
                        block,
                        params: &kind.params,
                        tint_for: &tint_for,
                        part_tint: &cell_part_tint,
                    };
                    // A family whose resolved form IS the material's full cube
                    // (a uniform full slab stack) falls to the cube path so it
                    // greedy-merges; the merge is load-bearing for streaming.
                    slab_as_cube = kind.render.meshes_as_cube(&ctx);
                    if !slab_as_cube {
                        mesh_boxes.clear();
                        kind.render.boxes(&ctx, &mut mesh_boxes);
                        // Nothing resolved (an unbaked Layer-3 cell) falls
                        // through to the cube path — the render fallback.
                        if !mesh_boxes.is_empty() {
                            apply_cell_tint(&mut mesh_boxes, section_idx(lx, ly, lz));
                            emit_box_set(
                                &mut opaque,
                                wx,
                                wy,
                                wz,
                                &mesh_boxes,
                                &mut box_scratch,
                                &neighbor_solid,
                                &neighbor_boxes,
                                &cell_matter,
                                &block_at,
                                &slab_at,
                                &neighbour_light,
                                &neighbour_blocklight,
                            );
                            continue;
                        }
                    }
                }

                if class & MODEL != 0 {
                    let kind = block
                        .model_kind()
                        .expect("a Model-family row carries its bbmodel kind");
                    let offset = section.model_offset(lx, ly, lz);
                    let facing = section.model_facing(lx, ly, lz);
                    let l = neighbour_light(wx, wy, wz) as u32;
                    let bl = neighbour_blocklight(wx, wy, wz).channels().map(u32::from);
                    let (sky6, blight) = fold_light(l, bl, SKY_FULL as u32);
                    emit_model_block(
                        &mut model,
                        &mut model_idx,
                        kind,
                        offset,
                        facing,
                        wx,
                        wy,
                        wz,
                        sky6,
                        blight,
                    );
                    // Contact shadow: only a BOTTOM footprint cell stamps, each
                    // single-cell piece (its own floor + its owned spill onto the
                    // dilation ring) gated on ITS stamped cell — an opaque full
                    // cube directly below, and no opaque full cube burying the
                    // floor at stamp level. Slabs, stairs, lowered cubes, glass,
                    // other models, and air get no stamp — supporting those
                    // shapes needs their real covered top surface and height,
                    // not a relaxed opacity check.
                    if offset[1] == 0 {
                        emit_model_contact(
                            &mut contact,
                            kind,
                            offset,
                            facing,
                            wx,
                            wy,
                            wz,
                            |gx, gz| {
                                let below = block_at(gx, wy - 1, gz);
                                if below.shape_family() != ShapeFamily::Cube || !below.is_opaque() {
                                    return false;
                                }
                                let at = block_at(gx, wy, gz);
                                at.shape_family() != ShapeFamily::Cube || !at.is_opaque()
                            },
                        );
                    }
                    continue;
                }

                // A cube-family cell with no exposed face draws nothing on the
                // fast path below, so skip its whole per-cell setup — tiles,
                // side style, log axis, front facing, water surface — instead
                // of computing all of it and then culling six faces. Buried
                // cells are the bulk of every underground section.
                let is_water = class & WATER != 0;
                let block_tiles = block.tiles();
                // Row-declared side treatments, resolved once per cell — the
                // mesher reads row fields, never concrete block ids. A
                // `covered_side` row (grass) swaps its sides to that tile while
                // a snow-cover block sits directly on top — derived from the
                // neighbour above at mesh time, so it heals itself the moment
                // the cover is placed or dug. Otherwise a `side_overlay` row
                // composites its base under the biome-tinted overlay (dirt +
                // grass overlay). `None` = the plain side tile.
                let side_style: Option<(Tile, Option<Tile>, [f32; 3])> = {
                    let covered = block
                        .covered_side()
                        .filter(|_| block_at(wx, wy + 1, wz).is_snow_cover());
                    match covered {
                        Some(t) => Some((t, None, tint_tile(t.world_tint(), ci))),
                        None => block.side_overlay().map(|so| {
                            (
                                so.base,
                                Some(so.overlay),
                                tint_tile(so.overlay.world_tint(), ci),
                            )
                        }),
                    }
                };
                let log_axis = if block.is_log() {
                    section.log_axis(lx, ly, lz)
                } else {
                    LogAxis::Y
                };
                // A directional-front row (furnace, lit furnace) draws its
                // `front` tile on the face its stored entity facing points to;
                // the other sides keep the plain side tile. The lit furnace is
                // its own block row, so "lit" is just this row read.
                let front_faces = block
                    .front_tile()
                    .map(|front| (facing_face(section.entity_facing(lx, ly, lz)), front));
                let base_x = wx as f32;
                let base_z = wz as f32;
                let base_y = wy as f32;

                // The cell's own `fills_cell` answer — cheap, and the ONLY
                // thing the water-vs-water cull needs. The full surface
                // resolve behind it (sixteen corner-height samples plus a flow
                // gradient) is DEFERRED to the first face that survives
                // culling: a submerged ocean cell draws nothing at all, and
                // those are the overwhelming majority of water cells.
                let water_full = is_water.then(|| match pad {
                    Some(pad) => pad.water_fills_local(lx as i32, ly as i32, lz as i32),
                    None => water_fills_cell(wx, wy, wz),
                });
                // A SUBMERGED water cell — full to the top, with six water
                // neighbours that are themselves full — draws nothing at all:
                // top and bottom cull against water outright, and each side
                // culls because the neighbour is not recessed. Ocean interiors
                // are the bulk of every water cell in the world, so testing it
                // once beats walking six faces to reach six culls.
                if water_full == Some(true) {
                    let nb_full = |dx: i32, dy: i32, dz: i32| match pad {
                        Some(pad) => {
                            pad.water_fills_local(lx as i32 + dx, ly as i32 + dy, lz as i32 + dz)
                        }
                        None => water_fills_cell(wx + dx, wy + dy, wz + dz),
                    };
                    if FACES.iter().all(|f| {
                        let (dx, dy, dz) = f.dir();
                        nb_full(dx, dy, dz)
                    }) {
                        continue;
                    }
                }

                let water_cell: std::cell::OnceCell<WaterSurface> = std::cell::OnceCell::new();
                let water_surface = || {
                    water_cell.get_or_init(|| {
                        let full = water_full.expect("only a water cell resolves a surface");
                        if let Some(pad) = pad {
                            // Pad-local samples: ±1 neighbours stay inside SECTION_PAD.
                            let (plx, ply, plz) = (lx as i32, ly as i32, lz as i32);
                            let block_l = |nwx, nwy, nwz| {
                                pad.block_local(plx + nwx - wx, ply + nwy - wy, plz + nwz - wz)
                            };
                            let fluid_l = |nwx, nwy, nwz| {
                                pad.fluid_height_local(
                                    plx + nwx - wx,
                                    ply + nwy - wy,
                                    plz + nwz - wz,
                                )
                            };
                            let still_l = |nwx, nwy, nwz| {
                                pad.water_still_local(
                                    plx + nwx - wx,
                                    ply + nwy - wy,
                                    plz + nwz - wz,
                                )
                            };
                            WaterSurface::new(wx, wy, wz, full, &block_l, &fluid_l, &still_l)
                        } else {
                            WaterSurface::new(
                                wx,
                                wy,
                                wz,
                                full,
                                &block_at,
                                &fluid_at,
                                &water_still_at,
                            )
                        }
                    })
                };

                if let (Some(pad), Some(exposed)) = (pad, exposed_masks.as_ref()) {
                    if class & FAST_CUBE != 0 || slab_as_cube {
                        let cell = section_idx(lx, ly, lz);
                        for face in FACES {
                            if !mask_has(exposed, face, cell) {
                                continue;
                            }
                            let is_side =
                                matches!(face, Face::PosX | Face::NegX | Face::PosZ | Face::NegZ);
                            let (base_tile, overlay_tile, tint) = match side_style {
                                Some(style) if is_side => style,
                                _ => {
                                    let t = cube_face_tile(
                                        block,
                                        face,
                                        block_tiles,
                                        front_faces,
                                        log_axis,
                                    );
                                    let tint = tint_tile(t.world_tint(), ci);
                                    (t, None, tint)
                                }
                            };
                            let tint = kv_tint(cell, tint);
                            let (dx, dy, dz) = face.dir();
                            let (fxp, fyp, fzp) = (
                                (lx as i32 + 1 + dx) as usize,
                                (ly as i32 + 1 + dy) as usize,
                                (lz as i32 + 1 + dz) as usize,
                            );
                            let fpi = mesh_pad_idx(fxp, fyp, fzp);
                            let f_l = pad.skylight[fpi] as u32;
                            let f_bl = pad.blocklight[fpi];
                            let (overlay, has_overlay) = match overlay_tile {
                                Some(o) => (o.index() as u32, true),
                                None => (0, false),
                            };
                            // Asked corner-free: a face bound for the greedy
                            // merge never builds its quad at all (the merged
                            // quad rebuilds one for the whole run).
                            let log_uvs_apply = log_side_uvs_apply(log_axis, face);
                            let (ao, light6, block6) = cube_face_lighting_pad(
                                pad,
                                face,
                                fxp,
                                fyp,
                                fzp,
                                (wx + dx, wy + dy, wz + dz),
                                f_l,
                                f_bl,
                                true,
                                &cell_matter,
                            );
                            let flat = ao[0] == ao[1]
                                && ao[1] == ao[2]
                                && ao[2] == ao[3]
                                && light6[0] == light6[1]
                                && light6[1] == light6[2]
                                && light6[2] == light6[3]
                                && block6[0] == block6[1]
                                && block6[1] == block6[2]
                                && block6[2] == block6[3];
                            if overlay_tile.is_none()
                                && (block.is_opaque() || slab_as_cube)
                                && flat
                                && !log_uvs_apply
                            {
                                let fi = face_index(face);
                                greedy.faces[fi * SECTION_VOLUME + cell] = FlatFace {
                                    gen: greedy_gen,
                                    // Dyed flag in bit 31 (part of the merge key).
                                    tile: base_tile.index() as u32
                                        | ((cell_tinted(cell) as u32) << 31),
                                    shade: FlatFace::shade(ao[0], light6[0], block6[0]),
                                    tint: block6[0].tint_word(tint),
                                };
                                let s = [lx, ly, lz][face_axes(face).0];
                                greedy.slice_counts[fi * SECTION_SIZE + s] += 1;
                            } else {
                                let corners = quad_for(face, base_x, base_y, base_z);
                                let log_uvs = log_side_cell_uvs(
                                    log_axis,
                                    face,
                                    corners,
                                    [base_x, base_y, base_z],
                                );
                                push_cube_face_with_cell_uvs(
                                    &mut opaque,
                                    corners,
                                    base_tile,
                                    overlay,
                                    has_overlay,
                                    UV_MODE_NONE,
                                    log_uvs,
                                    tint,
                                    face,
                                    ao,
                                    light6,
                                    block6,
                                    cell_tinted(cell),
                                );
                            }
                        }
                        continue;
                    }
                }

                for face in FACES {
                    let (dx, dy, dz) = face.dir();
                    let nwx = wx + dx;
                    let nwy = wy + dy;
                    let nwz = wz + dz;
                    let nb = block_at(nwx, nwy, nwz);

                    let is_water_top = is_water && matches!(face, Face::PosY);
                    let is_side = matches!(face, Face::PosX | Face::NegX | Face::PosZ | Face::NegZ);
                    let nb_solid = nb.is_opaque()
                        || (nb.is_slab() && slab_full_at(nwx, nwy, nwz))
                        || (matches!(face, Face::PosY) && seals_floor(IVec3::new(nwx, nwy, nwz)));
                    if nb_solid && !is_water_top {
                        continue;
                    }
                    if is_water && is_side && !neighbour_loaded(nwx, nwy, nwz) {
                        continue;
                    }
                    // A block that MERGES WITH ITSELF draws no interior face
                    // against its own kind: a glass wall reads as one pane
                    // rather than stacked frames, and an ice sheet as one
                    // volume rather than double-blended slabs. Leaves opt out
                    // — their interior faces are the canopy's depth — except
                    // under the Simplified leaf LOD, which asks for exactly
                    // this cull.
                    let merges = block.merges_with_self()
                        || (options.leaf_mesh_mode == LeafMeshMode::Simplified
                            && block.is_leaves());
                    if merges && nb == block {
                        continue;
                    }
                    let mut water_exposed_step = false;
                    if let Some(full) = water_full {
                        if nb == Block::Water {
                            let nb_full = if let Some(pad) = pad {
                                pad.water_fills_local(
                                    lx as i32 + dx,
                                    ly as i32 + dy,
                                    lz as i32 + dz,
                                )
                            } else {
                                water_fills_cell(nwx, nwy, nwz)
                            };
                            match water::side_vs_water(full, is_side, nb_full) {
                                SideVsWater::ExposedStep => water_exposed_step = true,
                                SideVsWater::Cull => continue,
                            }
                        }
                    }

                    let (base_tile, overlay_tile, tint) = if is_water {
                        let t = match face {
                            Face::PosY => water_surface().top_tile(),
                            Face::NegY => crate::atlas::engine().water_still,
                            // A STILL SOURCE's side faces are calm water — the
                            // step walls of the recessed pocket under a block
                            // sitting in the sea must not stream. Flowing and
                            // falling cells keep the animated flow sides.
                            _ if pad
                                .map(|p| p.water_still_local(lx as i32, ly as i32, lz as i32))
                                .unwrap_or_else(|| water_still_at(wx, wy, wz)) =>
                            {
                                crate::atlas::engine().water_still
                            }
                            _ => crate::atlas::engine().water_flow,
                        };
                        (t, None, tint_water(ci))
                    } else if let (true, Some(style)) = (is_side, side_style) {
                        style
                    } else {
                        let t = cube_face_tile(block, face, block_tiles, front_faces, log_axis);
                        let tint = tint_tile(t.world_tint(), ci);
                        (t, None, tint)
                    };
                    let tint = kv_tint(section_idx(lx, ly, lz), tint);

                    let mut corners = quad_for(face, base_x, base_y, base_z);
                    if is_water {
                        water_surface().warp_quad(
                            &mut corners,
                            base_x,
                            base_y,
                            base_z,
                            water_exposed_step,
                        );
                    }

                    let fx = nwx;
                    let fy = nwy;
                    let fz = nwz;
                    let f_l = neighbour_light(fx, fy, fz) as u32;
                    let f_bl = neighbour_blocklight(fx, fy, fz);

                    let water_ov: u32 = if is_water && matches!(face, Face::PosY) {
                        water_surface().top_angle()
                    } else {
                        0
                    };
                    let (overlay, has_overlay) = match overlay_tile {
                        Some(o) => (o.index() as u32, true),
                        None => (water_ov, false),
                    };
                    let log_uvs =
                        log_side_cell_uvs(log_axis, face, corners, [base_x, base_y, base_z]);

                    let (ao, light6, block6) = cube_face_lighting(
                        face,
                        fx,
                        fy,
                        fz,
                        boundary_plane(face, (fx, fy, fz)),
                        f_l,
                        f_bl,
                        true,
                        &block_at,
                        &slab_at,
                        &neighbour_light,
                        &neighbour_blocklight,
                        &cell_matter,
                    );
                    // Defer PLAIN opaque cube faces that are FLAT (all four corners share
                    // AO + every light channel) to the greedy merge — a run of them collapses into
                    // one tiled quad, pixel-identical. Water / grass-side (overlay) / leaves /
                    // cactus and any gradient (non-flat) face emit per-cell here, unchanged.
                    let flat = ao[0] == ao[1]
                        && ao[1] == ao[2]
                        && ao[2] == ao[3]
                        && light6[0] == light6[1]
                        && light6[1] == light6[2]
                        && light6[2] == light6[3]
                        && block6[0] == block6[1]
                        && block6[1] == block6[2]
                        && block6[2] == block6[3];
                    if !is_water
                        && overlay_tile.is_none()
                        && (block.is_opaque() || slab_as_cube)
                        && flat
                        && log_uvs.is_none()
                    {
                        let fi = face_index(face);
                        greedy.faces[fi * SECTION_VOLUME + section_idx(lx, ly, lz)] = FlatFace {
                            gen: greedy_gen,
                            // Dyed flag in bit 31 (part of the merge key).
                            tile: base_tile.index() as u32
                                | ((cell_tinted(section_idx(lx, ly, lz)) as u32) << 31),
                            shade: FlatFace::shade(ao[0], light6[0], block6[0]),
                            tint: block6[0].tint_word(tint),
                        };
                        // Slice index = the cell's coord along this face's normal axis.
                        let s = [lx, ly, lz][face_axes(face).0];
                        greedy.slice_counts[fi * SECTION_SIZE + s] += 1;
                    } else {
                        // Translucent blocks (ice) blend in their own
                        // depth-writing pass; their texels sit below the
                        // opaque pass's cutout and would discard to nothing
                        // there, and water's read-only depth cannot resolve a
                        // translucent cube sheet's own face order.
                        // Water TOP faces are the only two-sided terrain quads
                        // that stay in one draw: they go to their own cull-none
                        // stream instead of duplicating their vertices.
                        let vbuf = if is_water {
                            if matches!(face, Face::PosY) {
                                &mut transparent_two_sided
                            } else {
                                &mut transparent
                            }
                        } else if block.is_translucent() {
                            &mut translucent
                        } else {
                            &mut opaque
                        };
                        push_cube_face_with_cell_uvs(
                            vbuf,
                            corners,
                            base_tile,
                            overlay,
                            has_overlay,
                            UV_MODE_NONE,
                            log_uvs,
                            tint,
                            face,
                            ao,
                            light6,
                            block6,
                            cell_tinted(section_idx(lx, ly, lz)),
                        );
                    }
                }
            }
        }
    }

    // Collapse the deferred flat faces into merged tiled quads, then return the scratch to
    // the thread-local for the next section.
    emit_greedy_quads(&mut greedy, &mut opaque, ox, oy, oz);
    GREEDY.with(|g| *g.borrow_mut() = greedy);

    ChunkMesh {
        opaque,
        transparent,
        transparent_two_sided,
        translucent,
        model,
        model_idx,
        contact,
        mesh_dirty: true,
        ..ChunkMesh::empty()
    }
}

/// The chunk mesher's view of the primitive shape seam.
///
/// A mesh job runs on a WORKER thread over a padded section snapshot and has
/// no `&World`; that is the reason box producers were once duplicated per
/// consumer. Wrapping the pad's neighbour closures in `ShapeNeighborhood`
/// lets a shape family resolve here through exactly the seam it uses on the
/// sim thread, so one implementation serves both.
struct MeshNeighborhood<'a, B, S> {
    block: &'a B,
    cell_state: &'a S,
    section: &'a Section,
    origin: (i32, i32, i32),
}

impl<B, S> crate::block::ShapeNeighborhood for MeshNeighborhood<'_, B, S>
where
    B: Fn(i32, i32, i32) -> u8,
    S: Fn(i32, i32, i32) -> crate::block::ShapeState,
{
    fn block(&self, pos: IVec3) -> Block {
        Block::from_id((self.block)(pos.x, pos.y, pos.z))
    }

    fn shape_state(&self, pos: IVec3) -> crate::block::ShapeState {
        // ONE read of the unified store's pad capture — the seam ships the
        // bytes verbatim; only the family owning the cell's block decodes.
        (self.cell_state)(pos.x, pos.y, pos.z)
    }

    fn baked(&self, pos: IVec3) -> Option<&[crate::block::ShapeRenderBox]> {
        // Only this section's bakes are in the snapshot; a custom neighbour
        // across the boundary reads as unbaked, which means "no sub-cell
        // cull" — never a wrong one.
        let (ox, oy, oz) = self.origin;
        let (lx, ly, lz) = (pos.x - ox, pos.y - oy, pos.z - oz);
        let r = 0..SECTION_SIZE as i32;
        if !(r.contains(&lx) && r.contains(&ly) && r.contains(&lz)) {
            return None;
        }
        self.section
            .shape_render_boxes(section_idx(lx as usize, ly as usize, lz as usize) as u16)
    }
}
