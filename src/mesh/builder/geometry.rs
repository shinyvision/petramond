use glam::IVec3;

use crate::atlas::Tile;
use crate::block::{Block, ShapeFamily};
use crate::block_state::{LogAxis, SlabState, StairState};
use crate::chunk::{section_idx, SectionPos, SECTION_SIZE, SECTION_VOLUME, SKY_FULL};
use crate::section::Section;
use crate::torch::warm_tint;

use super::super::face::{cactus_quad, quad_for, Face, FACES};
use super::super::face_emit::{cube_face_lighting_pad, fold_light, push_cube_face_with_cell_uvs};
use super::super::greedy::{emit_greedy_quads, FlatFace, GreedyScratch, GREEDY};
use super::super::tint;
use super::super::vertex::{pack_tint, ChunkMesh, ModelVertex, UV_MODE_NONE};
use super::super::water::{self, SideVsWater, WaterSurface};

use super::cube_face::{
    cube_face_lighting, cube_face_tile, face_axes, face_index, facing_face, log_side_cell_uvs,
};
use super::exposed_masks::{build_exposed_masks, mask_has, pad_cube_fast_candidate};
use super::model_block::{emit_model_block, emit_model_contact};
use super::pad::{mesh_pad_idx, SectionMeshPad};
use super::super::boxset::{emit_box_set, BoxSetScratch, MeshBox};
use super::plant::emit_plant;
use super::{LeafMeshMode, MeshOptions};

#[allow(clippy::too_many_arguments)]
pub(super) fn section_geometry(
    section: &Section,
    pos: SectionPos,
    neighbour_block: impl Fn(i32, i32, i32) -> u8,
    neighbour_stair_state: impl Fn(i32, i32, i32) -> StairState,
    neighbour_slab_state: impl Fn(i32, i32, i32) -> SlabState,
    neighbour_water: impl Fn(i32, i32, i32) -> u8,
    neighbour_light: impl Fn(i32, i32, i32) -> u8,
    neighbour_blocklight: impl Fn(i32, i32, i32) -> u8,
    neighbour_loaded: impl Fn(i32, i32, i32) -> bool,
    tints: Option<&tint::BiomeTints>,
    options: MeshOptions,
    pad: Option<&SectionMeshPad<'_>>,
) -> ChunkMesh {
    let mut opaque = vec![];
    let mut opaque_idx = vec![];
    let mut transparent = vec![];
    let mut transparent_idx = vec![];
    let mut translucent = vec![];
    let mut translucent_idx = vec![];
    let mut model: Vec<ModelVertex> = vec![];
    let mut model_idx: Vec<u32> = vec![];
    let mut contact: Vec<super::super::vertex::ContactShadowVertex> = vec![];

    let (ox, oy, oz) = pos.origin_world();
    let tint_tile = |kind, ci| tints.map_or(tint::NO_TINT, |t| t.tile(kind, ci));
    let tint_water = |ci| tints.map_or(tint::NO_TINT, |t| t.water[ci]);

    // Every block read is by world coord through the routing closure (in-section
    // and cross-section alike); out-of-world / unloaded reads return air.
    let block_at =
        |wx: i32, wy: i32, wz: i32| -> Block { Block::from_id(neighbour_block(wx, wy, wz)) };
    let slab_at = |wx: i32, wy: i32, wz: i32| -> Option<SlabState> {
        let block = block_at(wx, wy, wz);
        crate::slab::is_slab(block)
            .then(|| crate::slab::normalize_state(block, neighbour_slab_state(wx, wy, wz)))
    };
    // "Cell holds a full slab stack" — callers gate on `is_slab` first (dense flag)
    // so this only pays a state lookup on actual slab cells. Full stacks cull and
    // occlude AO/light like opaque cubes; no normalize needed (a normalized default
    // is a single layer, never full).
    let slab_full_at =
        |wx: i32, wy: i32, wz: i32| -> bool { neighbour_slab_state(wx, wy, wz).is_full() };
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

    // The unified box-set emitter's scratch + box buffer (see mesh::boxset):
    // every axis-aligned sub-cell shape family routes through it.
    let mut box_scratch = BoxSetScratch::default();
    let mut mesh_boxes: Vec<MeshBox> = Vec::new();

    // A neighbour stair's resolved corner shape — the neighbour-of-neighbour
    // read stair corner resolution, pane/fence connection, and occupancy
    // queries share.
    let stair_shape_at = |q: IVec3| {
        crate::stair::resolved_shape(q, neighbour_stair_state(q.x, q.y, q.z), |r| {
            crate::stair::is_stair(block_at(r.x, r.y, r.z))
                .then(|| neighbour_stair_state(r.x, r.y, r.z))
        })
    };

    // The cell-local occupancy boxes of the block at `p` — what the box-set
    // emitter subtracts from a flush face so sub-cell geometry culls against
    // sub-cell geometry (a fence cap on a slab, a chain continuing into the
    // chain above). Whole opaque cells are handled by the cheaper solid cull
    // and contribute nothing here; families with no box form (plants, torch,
    // models, custom bakes across the section boundary) stay empty, which
    // just means "no sub-cell cull", never a wrong cull.
    let occupancy_boxes = |p: IVec3, out: &mut Vec<([f32; 3], [f32; 3])>| {
        let nb = block_at(p.x, p.y, p.z);
        match nb.shape_family() {
            ShapeFamily::Stair => {
                let shape = stair_shape_at(p);
                out.extend(
                    crate::stair::boxes_for_shape(shape)
                        .iter()
                        .map(|a| (a.min, a.max)),
                );
            }
            ShapeFamily::Slab => {
                if let Some(state) = slab_at(p.x, p.y, p.z) {
                    for (slot, _) in crate::slab::layer_slots(state) {
                        out.push(super::super::slab::slot_box(slot));
                    }
                }
            }
            ShapeFamily::Ladder => {
                let (thickness, height) = nb.ladder_dims();
                out.push(crate::ladder::panel_aabb_dim(
                    nb.panel_facing(),
                    thickness,
                    height,
                ));
            }
            ShapeFamily::Fence | ShapeFamily::Pane => {
                if let Some(c) = nb.shape_kind_def().params.connection() {
                    let family = nb.shape_family();
                    let mask = crate::connect::resolved_mask(
                        p,
                        |q| block_at(q.x, q.y, q.z),
                        &stair_shape_at,
                        |q| slab_full_at(q.x, q.y, q.z),
                        |b, dir, st, sl| crate::connect::connects(c.rule, family, b, dir, st, sl),
                    );
                    if family == ShapeFamily::Fence {
                        super::super::fence::shape_boxes(c.post_lo, c.post_hi, mask, |mn, mx, _| {
                            out.push((mn, mx))
                        });
                    } else {
                        super::super::pane::shape_boxes(c.post_lo, c.post_hi, mask, |mn, mx, _| {
                            out.push((mn, mx))
                        });
                    }
                }
            }
            _ => {}
        }
    };

    // The shared sub-cell AO occupancy query: does the cell hold solid matter
    // overlapping the cell-local pocket AABB? Whole cell for opaque cubes /
    // full stacks;
    // half-cell state for partial slabs and stairs (the UNRESOLVED state
    // shape — corner-join resolution reads the stair's own neighbours, which
    // the pad mesher cannot reach, and the two meshers must agree
    // byte-for-byte); the mask-free post for fences/panes (mask resolution is
    // likewise out of the pad's reach — rails are thin and corner-distant
    // anyway); the panel for ladders; the render-bake boxes for IN-SECTION
    // custom cells (both meshers share `section`, so the restriction is
    // parity-safe). Consumed by the cube gathers' cast probes AND the box
    // emitter's out-of-cell probes, so casting and receiving are one rule.
    let cell_matter = |cl: (i32, i32, i32), lo: [f32; 3], hi: [f32; 3]| -> bool {
        let (cx, cy, cz) = cl;
        let b = block_at(cx, cy, cz);
        if b.occludes_ao() {
            return true;
        }
        // Which half-cell octants the pocket overlaps, ORed over occupancy.
        let any_octant = |occ: &dyn Fn(usize, usize, usize) -> bool| -> bool {
            let touches = |a: usize, half: usize| {
                if half == 0 {
                    lo[a] < 0.5
                } else {
                    hi[a] > 0.5
                }
            };
            (0..8).any(|o| {
                let (ix, iy, iz) = (o & 1, (o >> 1) & 1, (o >> 2) & 1);
                touches(0, ix) && touches(1, iy) && touches(2, iz) && occ(ix, iy, iz)
            })
        };
        if b.is_slab() {
            return slab_at(cx, cy, cz).is_some_and(|state| {
                state.is_full()
                    || any_octant(&|ix, iy, iz| crate::slab::half_cell_occupied(state, ix, iy, iz))
            });
        }
        let overlaps = |mn: [f32; 3], mx: [f32; 3]| (0..3).all(|a| lo[a] < mx[a] && hi[a] > mn[a]);
        match b.shape_family() {
            ShapeFamily::Stair => {
                let shape = crate::stair::shape(neighbour_stair_state(cx, cy, cz));
                any_octant(&|ix, iy, iz| crate::stair::shape_half_cell_occupied(shape, ix, iy, iz))
            }
            ShapeFamily::Fence | ShapeFamily::Pane => {
                b.shape_kind_def().params.connection().is_some_and(|c| {
                    lo[0] < c.post_hi && hi[0] > c.post_lo && lo[2] < c.post_hi && hi[2] > c.post_lo
                })
            }
            ShapeFamily::Ladder => {
                let (thickness, height) = b.ladder_dims();
                let (mn, mx) = crate::ladder::panel_aabb_dim(b.panel_facing(), thickness, height);
                overlaps(mn, mx)
            }
            ShapeFamily::Custom => {
                let (lx, ly, lz) = (cx - ox, cy - oy, cz - oz);
                let range = 0..SECTION_SIZE as i32;
                range.contains(&lx)
                    && range.contains(&ly)
                    && range.contains(&lz)
                    && section
                        .shape_render_boxes(
                            section_idx(lx as usize, ly as usize, lz as usize) as u16
                        )
                        .is_some_and(|boxes| boxes.iter().any(|bx| overlaps(bx.min, bx.max)))
            }
            _ => false,
        }
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
        .map(build_exposed_masks);

    for ly in 0..SECTION_SIZE {
        for lz in 0..SECTION_SIZE {
            for lx in 0..SECTION_SIZE {
                let id = section.block_raw(lx, ly, lz);
                let block = Block::from_id(id);
                if block == Block::Air {
                    continue;
                }
                if block == Block::Chest {
                    continue;
                }
                // Resolve the render shape once per cell (each call indexes the block
                // table); the special-shape checks below and the cube fallthrough share it.
                let shape = block.shape_family();
                if shape == ShapeFamily::Door {
                    continue;
                }

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
                    occupancy_boxes(IVec3::new(wx + dx, wy + dy, wz + dz), out);
                };

                if matches!(shape, ShapeFamily::Cross | ShapeFamily::Crop) {
                    let tile = block.tiles()[0];
                    let l = neighbour_light(wx, wy, wz) as u32;
                    let bl = neighbour_blocklight(wx, wy, wz) as u32;
                    let (sky6, block6, warm) = fold_light(l, bl, SKY_FULL as u32);
                    let tint = warm_tint(tint_tile(tile.world_tint(), ci), warm);
                    // Layer-2 dimensions (a mod's retuned cross/crop) or the
                    // engine defaults for a parameterless row.
                    let dims = block.shape_kind_def().params.dimensions();
                    let (inset, drop) = if shape == ShapeFamily::Crop {
                        (
                            dims.map_or(crate::block::CROP_PLANE_INSET, |d| d.inset),
                            dims.map_or(crate::block::CROP_PLANE_DROP, |d| d.drop),
                        )
                    } else {
                        (dims.map_or(0.0, |d| d.inset), 0.0)
                    };
                    emit_plant(
                        &mut opaque,
                        &mut opaque_idx,
                        shape,
                        wx as f32,
                        wy as f32,
                        wz as f32,
                        tile,
                        tint,
                        sky6,
                        block6,
                        inset,
                        drop,
                    );
                    continue;
                }

                if shape == ShapeFamily::Torch {
                    let [top_tile, _bottom, side_tile] = block.tiles();
                    // Sky channel = the cell's skylight; block channel = the torch's own
                    // emission (self-lit). `max(sky_term, block_term)` in the shader
                    // equals the old single-channel `max(cell_sky, emission)` fold at
                    // identity scale, and the emission channel never dims at night.
                    let cell_sky = neighbour_light(wx, wy, wz) as u32;
                    let sky6 = ((cell_sky * 63 + SKY_FULL as u32 / 2) / SKY_FULL as u32).min(63);
                    let emit = block.light_emission() as u32;
                    let block6 = ((emit * 63 + SKY_FULL as u32 / 2) / SKY_FULL as u32).min(63);
                    let placement = section.torch_placement(lx, ly, lz);
                    super::torch::emit_torch(
                        &mut opaque,
                        &mut opaque_idx,
                        wx as f32,
                        wy as f32,
                        wz as f32,
                        placement,
                        side_tile,
                        top_tile,
                        [1.0, 1.0, 1.0],
                        sky6,
                        block6,
                    );
                    continue;
                }

                if shape == ShapeFamily::Ladder {
                    let tile = block.tiles()[0];
                    // The facing is the ROW's (one block row per facing) —
                    // the mesher reads row fields, never per-cell maps.
                    let facing = block.panel_facing();
                    let (thickness, height) = block.ladder_dims();
                    mesh_boxes.clear();
                    super::super::ladder::push_mesh_box(
                        &mut mesh_boxes,
                        facing,
                        thickness,
                        height,
                        tile,
                        tint_tile(tile.world_tint(), ci),
                    );
                    emit_box_set(
                        &mut opaque,
                        &mut opaque_idx,
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

                if let Some(kind) = block.model_kind() {
                    let offset = section.model_offset(lx, ly, lz);
                    let facing = section.model_facing(lx, ly, lz);
                    let l = neighbour_light(wx, wy, wz) as u32;
                    let bl = neighbour_blocklight(wx, wy, wz) as u32;
                    let (sky6, block6, warm) = fold_light(l, bl, SKY_FULL as u32);
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
                        block6,
                        warm,
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

                if shape == ShapeFamily::Stair {
                    let tiles = block.tiles();
                    let tint_for = |tile: Tile| tint_tile(tile.world_tint(), ci);
                    let shape = crate::stair::resolved_shape(
                        IVec3::new(wx, wy, wz),
                        section.stair_state(lx, ly, lz),
                        |p| {
                            crate::stair::is_stair(block_at(p.x, p.y, p.z))
                                .then(|| neighbour_stair_state(p.x, p.y, p.z))
                        },
                    );
                    mesh_boxes.clear();
                    mesh_boxes.extend(
                        crate::stair::boxes_for_shape(shape)
                            .iter()
                            .map(|a| MeshBox::uniform(a.min, a.max, tiles, tint_for)),
                    );
                    emit_box_set(
                        &mut opaque,
                        &mut opaque_idx,
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

                if shape == ShapeFamily::Pane {
                    // [top, bottom, side] tiles = [edge, edge, glass].
                    let [edge_tile, _bottom, glass_tile] = block.tiles();
                    // Post dimensions + connection rule are shape-kind params, so a
                    // modded bar/wall's post and connection behaviour ride here.
                    let c = block
                        .shape_kind_def()
                        .params
                        .connection()
                        .expect("pane carries connection params");
                    let mask = crate::connect::resolved_mask(
                        IVec3::new(wx, wy, wz),
                        |q| block_at(q.x, q.y, q.z),
                        &stair_shape_at,
                        |q| slab_full_at(q.x, q.y, q.z),
                        |nb, dir, st, sl| {
                            crate::connect::connects(c.rule, ShapeFamily::Pane, nb, dir, st, sl)
                        },
                    );
                    mesh_boxes.clear();
                    super::super::pane::push_mesh_boxes(
                        &mut mesh_boxes,
                        c.post_lo,
                        c.post_hi,
                        mask,
                        glass_tile,
                        edge_tile,
                        tint_tile(glass_tile.world_tint(), ci),
                    );
                    emit_box_set(
                        &mut opaque,
                        &mut opaque_idx,
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

                if shape == ShapeFamily::Fence {
                    let tiles = block.tiles();
                    // Post dimensions + connection rule are shape-kind params, so a
                    // modded wall's post and connection behaviour ride here without
                    // touching the mesher.
                    let c = block
                        .shape_kind_def()
                        .params
                        .connection()
                        .expect("fence carries connection params");
                    let mask = crate::connect::resolved_mask(
                        IVec3::new(wx, wy, wz),
                        |q| block_at(q.x, q.y, q.z),
                        &stair_shape_at,
                        |q| slab_full_at(q.x, q.y, q.z),
                        |nb, dir, st, sl| {
                            crate::connect::connects(c.rule, ShapeFamily::Fence, nb, dir, st, sl)
                        },
                    );
                    mesh_boxes.clear();
                    super::super::fence::push_mesh_boxes(
                        &mut mesh_boxes,
                        c.post_lo,
                        c.post_hi,
                        mask,
                        tiles,
                        tint_tile(tiles[2].world_tint(), ci),
                    );
                    emit_box_set(
                        &mut opaque,
                        &mut opaque_idx,
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

                // A Layer-3 custom shape emits the boxes the client render bake
                // produced (cached on the section); a cache miss / pre-bake / trap
                // falls through to the cube path (the render fallback).
                if shape == ShapeFamily::Custom {
                    if let Some(boxes) = section.shape_render_boxes(section_idx(lx, ly, lz) as u16) {
                        let tiles = block.tiles();
                        let tint_for = |tile: Tile| tint_tile(tile.world_tint(), ci);
                        mesh_boxes.clear();
                        mesh_boxes.extend(
                            boxes
                                .iter()
                                .map(|a| MeshBox::uniform(a.min, a.max, tiles, tint_for)),
                        );
                        emit_box_set(
                            &mut opaque,
                            &mut opaque_idx,
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

                // A same-material full slab stack IS the material's full cube: fall
                // through to the cube path (fast path + greedy merge included) so it
                // culls, lights, and merges like one. Partial cells and mixed-material
                // full stacks keep the per-layer emitter (preserving each layer's
                // texture); full stacks of either kind still cull/occlude as opaque
                // via `slab_full_at`.
                let mut slab_as_cube = false;
                if shape == ShapeFamily::Slab {
                    let state = crate::slab::normalize_state(block, section.slab_state(lx, ly, lz));
                    slab_as_cube = crate::slab::is_uniform_full_stack(state);
                    if !slab_as_cube {
                        let tint_for = |tile: Tile| tint_tile(tile.world_tint(), ci);
                        mesh_boxes.clear();
                        for (slot, layer_block) in crate::slab::layer_slots(state) {
                            let (min, max) = super::super::slab::slot_box(slot);
                            mesh_boxes.push(MeshBox::uniform(
                                min,
                                max,
                                layer_block.tiles(),
                                tint_for,
                            ));
                        }
                        emit_box_set(
                            &mut opaque,
                            &mut opaque_idx,
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

                let is_water = block == Block::Water;
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

                let water_surface = is_water.then(|| {
                    if let Some(pad) = pad {
                        // Pad-local samples: ±1 neighbours stay inside SECTION_PAD.
                        let (plx, ply, plz) = (lx as i32, ly as i32, lz as i32);
                        let full = pad.water_fills_local(plx, ply, plz);
                        let block_l =
                            |nwx, nwy, nwz| pad.block_local(plx + nwx - wx, ply + nwy - wy, plz + nwz - wz);
                        let fluid_l = |nwx, nwy, nwz| {
                            pad.fluid_height_local(plx + nwx - wx, ply + nwy - wy, plz + nwz - wz)
                        };
                        let still_l = |nwx, nwy, nwz| {
                            pad.water_still_local(plx + nwx - wx, ply + nwy - wy, plz + nwz - wz)
                        };
                        WaterSurface::new(wx, wy, wz, full, &block_l, &fluid_l, &still_l)
                    } else {
                        let full = water_fills_cell(wx, wy, wz);
                        WaterSurface::new(wx, wy, wz, full, &block_at, &fluid_at, &water_still_at)
                    }
                });

                if let (Some(pad), Some(exposed)) = (pad, exposed_masks.as_ref()) {
                    if pad_cube_fast_candidate(block) || slab_as_cube {
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
                            let corners = quad_for(face, base_x, base_y, base_z);
                            let (dx, dy, dz) = face.dir();
                            let (fxp, fyp, fzp) = (
                                (lx as i32 + 1 + dx) as usize,
                                (ly as i32 + 1 + dy) as usize,
                                (lz as i32 + 1 + dz) as usize,
                            );
                            let fpi = mesh_pad_idx(fxp, fyp, fzp);
                            let f_l = pad.skylight[fpi] as u32;
                            let f_bl = pad.blocklight[fpi] as u32;
                            let (overlay, has_overlay) = match overlay_tile {
                                Some(o) => (o.index() as u32, true),
                                None => (0, false),
                            };
                            let log_uvs = log_side_cell_uvs(
                                log_axis,
                                face,
                                corners,
                                [base_x, base_y, base_z],
                            );
                            let (ao, light6, block6, warm) = cube_face_lighting_pad(
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
                                && block6[2] == block6[3]
                                && warm[0] == warm[1]
                                && warm[1] == warm[2]
                                && warm[2] == warm[3];
                            if overlay_tile.is_none()
                                && (block.is_opaque() || slab_as_cube)
                                && flat
                                && log_uvs.is_none()
                            {
                                let final_tint = if warm[0] == 0.0 {
                                    tint
                                } else {
                                    warm_tint(tint, warm[0])
                                };
                                let fi = face_index(face);
                                greedy.faces[fi * SECTION_VOLUME + cell] = FlatFace {
                                    gen: greedy_gen,
                                    tile: base_tile.index() as u32,
                                    ao: ao[0],
                                    light6: light6[0],
                                    block6: block6[0],
                                    tint: pack_tint(final_tint),
                                };
                                let s = [lx, ly, lz][face_axes(face).0];
                                greedy.slice_counts[fi * SECTION_SIZE + s] += 1;
                            } else {
                                push_cube_face_with_cell_uvs(
                                    &mut opaque,
                                    &mut opaque_idx,
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
                                    warm,
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
                    let is_cactus_side = block == Block::Cactus && is_side;
                    // A lowered cube's top plane sits INSIDE the cell — nothing
                    // above can ever cover it, so it is exempt from the
                    // neighbour cull (the block-above's bottom face still draws
                    // since lowered rows are non-opaque: no x-ray slit).
                    let lowered = block.lowered_height();
                    let is_lowered_top = lowered.is_some() && matches!(face, Face::PosY);
                    // A lowered cube's full 1×1 base sits flush on the cell
                    // floor, so for the face BENEATH it it covers exactly like
                    // an opaque cube (a snow layer's carrier top would z-fight
                    // the layer from far above otherwise).
                    let nb_covers_below = matches!(face, Face::PosY) && nb.is_lowered_cube();
                    let nb_solid = nb.is_opaque()
                        || (nb.is_slab() && slab_full_at(nwx, nwy, nwz))
                        || nb_covers_below;
                    if nb_solid && !is_water_top && !is_cactus_side && !is_lowered_top {
                        continue;
                    }
                    if is_water && is_side && !neighbour_loaded(nwx, nwy, nwz) {
                        continue;
                    }
                    if options.leaf_mesh_mode == LeafMeshMode::Simplified
                        && block == Block::OakLeaves
                        && nb == Block::OakLeaves
                    {
                        continue;
                    }
                    // Two adjacent glass blocks share no visible face: cull both
                    // sides so a glass wall reads as one pane, not stacked frames.
                    if block == Block::Glass && nb == Block::Glass {
                        continue;
                    }
                    // Same rule for a translucent block against itself (ice
                    // against ice): interior faces would double-blend, so the
                    // frozen sheet reads as one volume, not stacked slabs.
                    if block.is_translucent() && nb == block {
                        continue;
                    }
                    // Two flush lowered cubes share no visible side either: the
                    // neighbour's body covers my whole (equally short) face.
                    if let (Some(h), Some(nh)) = (lowered, nb.lowered_height()) {
                        if is_side && nh >= h {
                            continue;
                        }
                    }
                    let mut water_exposed_step = false;
                    if let Some(ws) = &water_surface {
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
                            match ws.side_against_water(is_side, nb_full) {
                                SideVsWater::ExposedStep => water_exposed_step = true,
                                SideVsWater::Cull => continue,
                            }
                        }
                    }

                    let (base_tile, overlay_tile, tint) = if let Some(ws) = &water_surface {
                        let t = match face {
                            Face::PosY => ws.top_tile(),
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

                    let mut corners = if block == Block::Cactus {
                        cactus_quad(
                            face,
                            [base_x, base_y, base_z],
                            [base_x + 1.0, base_y + 1.0, base_z + 1.0],
                        )
                    } else {
                        quad_for(face, base_x, base_y, base_z)
                    };
                    if let Some(h) = lowered {
                        // Sink the visible top: the top face drops to h/16 and
                        // side faces shorten with it (full tile compressed a
                        // texel, like the cactus insets — no UV plumbing).
                        let top = base_y + h as f32 / 16.0;
                        for c in &mut corners {
                            c[1] = c[1].min(top);
                        }
                    }
                    if let Some(ws) = &water_surface {
                        ws.warp_quad(&mut corners, base_x, base_y, base_z, water_exposed_step);
                    }

                    let fx = nwx;
                    let fy = nwy;
                    let fz = nwz;
                    let f_l = neighbour_light(fx, fy, fz) as u32;
                    let f_bl = neighbour_blocklight(fx, fy, fz) as u32;

                    let water_ov: u32 = match &water_surface {
                        Some(ws) if matches!(face, Face::PosY) => ws.top_angle(),
                        _ => 0,
                    };
                    let (overlay, has_overlay) = match overlay_tile {
                        Some(o) => (o.index() as u32, true),
                        None => (water_ov, false),
                    };
                    let log_uvs =
                        log_side_cell_uvs(log_axis, face, corners, [base_x, base_y, base_z]);

                    let (ao, light6, block6, warm) = cube_face_lighting(
                        face,
                        fx,
                        fy,
                        fz,
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
                    // AO + light + warm) to the greedy merge — a run of them collapses into
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
                        && block6[2] == block6[3]
                        && warm[0] == warm[1]
                        && warm[1] == warm[2]
                        && warm[2] == warm[3];
                    if !is_water
                        && overlay_tile.is_none()
                        && (block.is_opaque() || slab_as_cube)
                        && flat
                        && log_uvs.is_none()
                    {
                        let final_tint = if warm[0] == 0.0 {
                            tint
                        } else {
                            warm_tint(tint, warm[0])
                        };
                        let fi = face_index(face);
                        greedy.faces[fi * SECTION_VOLUME + section_idx(lx, ly, lz)] = FlatFace {
                            gen: greedy_gen,
                            tile: base_tile.index() as u32,
                            ao: ao[0],
                            light6: light6[0],
                            block6: block6[0],
                            tint: pack_tint(final_tint),
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
                        let (vbuf, ibuf) = if is_water {
                            (&mut transparent, &mut transparent_idx)
                        } else if block.is_translucent() {
                            (&mut translucent, &mut translucent_idx)
                        } else {
                            (&mut opaque, &mut opaque_idx)
                        };
                        let tris = push_cube_face_with_cell_uvs(
                            vbuf,
                            ibuf,
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
                            warm,
                        );
                        if is_water && matches!(face, Face::PosY) {
                            ibuf.extend_from_slice(&water::top_back_winding(tris));
                        }
                    }
                }
            }
        }
    }

    // Collapse the deferred flat faces into merged tiled quads, then return the scratch to
    // the thread-local for the next section.
    emit_greedy_quads(&mut greedy, &mut opaque, &mut opaque_idx, ox, oy, oz);
    GREEDY.with(|g| *g.borrow_mut() = greedy);

    ChunkMesh {
        opaque,
        opaque_idx,
        transparent,
        transparent_idx,
        translucent,
        translucent_idx,
        model,
        model_idx,
        contact,
        mesh_dirty: true,
        ..ChunkMesh::empty()
    }
}
