use glam::IVec3;

use crate::atlas::Tile;
use crate::block::{Block, RenderShape};
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
use super::model_block::emit_model_block;
use super::pad::{mesh_pad_idx, SectionMeshPad};
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

    let (ox, oy, oz) = pos.origin_world();
    let tint_tile = |kind, ci| tints.map_or(tint::NO_TINT, |t| t.tile(kind, ci));
    let tint_grass = |ci| tints.map_or(tint::NO_TINT, |t| t.grass[ci]);
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
                let shape = block.render_shape();
                if shape == RenderShape::Door {
                    continue;
                }

                let wx = ox + lx as i32;
                let wy = oy + ly as i32;
                let wz = oz + lz as i32;
                let ci = lz * SECTION_SIZE + lx;

                if matches!(shape, RenderShape::Cross | RenderShape::Crop) {
                    let tile = block.tiles()[0];
                    let l = neighbour_light(wx, wy, wz) as u32;
                    let bl = neighbour_blocklight(wx, wy, wz) as u32;
                    let (sky6, block6, warm) = fold_light(l, bl, SKY_FULL as u32);
                    let tint = warm_tint(tint_tile(tile.world_tint(), ci), warm);
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
                    );
                    continue;
                }

                if shape == RenderShape::Torch {
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

                if shape == RenderShape::Ladder {
                    let tile = block.tiles()[0];
                    let l = neighbour_light(wx, wy, wz) as u32;
                    let bl = neighbour_blocklight(wx, wy, wz) as u32;
                    let (sky6, block6, warm) = fold_light(l, bl, SKY_FULL as u32);
                    let facing = section.entity_facing(lx, ly, lz);
                    super::ladder::emit_ladder_block(
                        &mut opaque,
                        &mut opaque_idx,
                        wx,
                        wy,
                        wz,
                        facing,
                        tile,
                        tint_tile(tile.world_tint(), ci),
                        sky6,
                        block6,
                        warm,
                    );
                    continue;
                }

                if let RenderShape::Model(kind) = shape {
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
                    continue;
                }

                if shape == RenderShape::Stair {
                    let [tile_top, tile_bot, tile_side] = block.tiles();
                    let tint_for = |tile: Tile| tint_tile(tile.world_tint(), ci);
                    let state = section.stair_state(lx, ly, lz);
                    let shape = crate::stair::resolved_shape(IVec3::new(wx, wy, wz), state, |p| {
                        crate::stair::is_stair(block_at(p.x, p.y, p.z))
                            .then(|| neighbour_stair_state(p.x, p.y, p.z))
                    });
                    super::stair::emit_stair_block(
                        &mut opaque,
                        &mut opaque_idx,
                        wx,
                        wy,
                        wz,
                        shape,
                        [tile_top, tile_bot, tile_side],
                        &tint_for,
                        &block_at,
                        &slab_at,
                        &neighbour_light,
                        &neighbour_blocklight,
                    );
                    continue;
                }

                if shape == RenderShape::Pane {
                    // [top, bottom, side] tiles = [edge, edge, glass].
                    let [edge_tile, _bottom, glass_tile] = block.tiles();
                    // A neighbour stair's resolved corner shape decides whether its
                    // face toward the pane is complete — same neighbour-of-neighbour
                    // read the stair's own corner resolution does.
                    let stair_shape_at = |q: IVec3| {
                        crate::stair::resolved_shape(q, neighbour_stair_state(q.x, q.y, q.z), |r| {
                            crate::stair::is_stair(block_at(r.x, r.y, r.z))
                                .then(|| neighbour_stair_state(r.x, r.y, r.z))
                        })
                    };
                    let pane_mask_at = |p: IVec3| {
                        crate::pane::resolved_mask(
                            p,
                            |q| block_at(q.x, q.y, q.z),
                            &stair_shape_at,
                            |q| slab_full_at(q.x, q.y, q.z),
                        )
                    };
                    let vertical = |dy: i32| {
                        let vb = block_at(wx, wy + dy, wz);
                        if vb == block {
                            super::pane::PaneVertical::Pane(pane_mask_at(IVec3::new(
                                wx,
                                wy + dy,
                                wz,
                            )))
                        } else if vb.is_opaque() || (vb.is_slab() && slab_full_at(wx, wy + dy, wz))
                        {
                            super::pane::PaneVertical::Solid
                        } else {
                            super::pane::PaneVertical::Open
                        }
                    };
                    let l = neighbour_light(wx, wy, wz) as u32;
                    let bl = neighbour_blocklight(wx, wy, wz) as u32;
                    let (sky6, block6, warm) = fold_light(l, bl, SKY_FULL as u32);
                    super::pane::emit_pane_block(
                        &mut opaque,
                        &mut opaque_idx,
                        wx,
                        wy,
                        wz,
                        pane_mask_at(IVec3::new(wx, wy, wz)),
                        vertical(1),
                        vertical(-1),
                        glass_tile,
                        edge_tile,
                        tint_tile(glass_tile.world_tint(), ci),
                        sky6,
                        block6,
                        warm,
                    );
                    continue;
                }

                // A same-material full slab stack IS the material's full cube: fall
                // through to the cube path (fast path + greedy merge included) so it
                // culls, lights, and merges like one. Partial cells and mixed-material
                // full stacks keep the per-layer emitter (preserving each layer's
                // texture); full stacks of either kind still cull/occlude as opaque
                // via `slab_full_at`.
                let mut slab_as_cube = false;
                if shape == RenderShape::Slab {
                    let state = crate::slab::normalize_state(block, section.slab_state(lx, ly, lz));
                    slab_as_cube = crate::slab::is_uniform_full_stack(state);
                    if !slab_as_cube {
                        let tint_for = |tile: Tile| tint_tile(tile.world_tint(), ci);
                        super::slab::emit_slab_block(
                            &mut opaque,
                            &mut opaque_idx,
                            wx,
                            wy,
                            wz,
                            state,
                            &tint_for,
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
                // Grass sides swap to the untinted snowy texture while a
                // snow-cover block (snow layer / snow block) sits directly on
                // top — derived from the neighbour above at mesh time, so it
                // heals itself the moment the cover is placed or dug.
                let grass_snow_covered =
                    block == Block::Grass && block_at(wx, wy + 1, wz).is_snow_cover();
                let log_axis = if block.is_log() {
                    section.log_axis(lx, ly, lz)
                } else {
                    LogAxis::Y
                };
                let furnace_faces = (block == Block::Furnace).then(|| {
                    let front = if section.is_furnace_lit(lx, ly, lz) {
                        crate::atlas::engine().furnace_front_on
                    } else {
                        crate::atlas::engine().furnace_front
                    };
                    (facing_face(section.entity_facing(lx, ly, lz)), front)
                });
                let base_x = wx as f32;
                let base_z = wz as f32;
                let base_y = wy as f32;

                let water_surface = is_water.then(|| {
                    let full = water_fills_cell(wx, wy, wz);
                    WaterSurface::new(wx, wy, wz, full, &block_at, &fluid_at, &water_still_at)
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
                            let (base_tile, overlay_tile, tint) =
                                if block == Block::Grass && is_side {
                                    let e = crate::atlas::engine();
                                    if grass_snow_covered {
                                        (e.grass_snow, None, tint_tile(e.grass_snow.world_tint(), ci))
                                    } else {
                                        (e.dirt, Some(e.grass_side_overlay), tint_grass(ci))
                                    }
                                } else {
                                    let t = cube_face_tile(
                                        block,
                                        face,
                                        block_tiles,
                                        furnace_faces,
                                        log_axis,
                                    );
                                    let tint = tint_tile(t.world_tint(), ci);
                                    (t, None, tint)
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
                            let (ao, light6, block6, warm) =
                                cube_face_lighting_pad(pad, face, fxp, fyp, fzp, f_l, f_bl, true);
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
                    let lowered = match shape {
                        RenderShape::LoweredCube(h) => Some(h),
                        _ => None,
                    };
                    let is_lowered_top = lowered.is_some() && matches!(face, Face::PosY);
                    let nb_solid = nb.is_opaque() || (nb.is_slab() && slab_full_at(nwx, nwy, nwz));
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
                    if let (Some(h), RenderShape::LoweredCube(nh)) = (lowered, nb.render_shape()) {
                        if is_side && nh >= h {
                            continue;
                        }
                    }
                    let mut water_exposed_step = false;
                    if let Some(ws) = &water_surface {
                        if nb == Block::Water {
                            let nb_full = water_fills_cell(nwx, nwy, nwz);
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
                            _ if water_still_at(wx, wy, wz) => {
                                crate::atlas::engine().water_still
                            }
                            _ => crate::atlas::engine().water_flow,
                        };
                        (t, None, tint_water(ci))
                    } else if block == Block::Grass && is_side {
                        let e = crate::atlas::engine();
                        if grass_snow_covered {
                            (e.grass_snow, None, tint_tile(e.grass_snow.world_tint(), ci))
                        } else {
                            (e.dirt, Some(e.grass_side_overlay), tint_grass(ci))
                        }
                    } else {
                        let t = cube_face_tile(block, face, block_tiles, furnace_faces, log_axis);
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
        mesh_dirty: true,
        ..ChunkMesh::empty()
    }
}
