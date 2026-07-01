use glam::{IVec3, Vec3};

use crate::atlas::Tile;
use crate::block::{Block, RenderShape};
use crate::block_model::{self, BlockModelKind};
#[cfg(test)]
use crate::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ};
use crate::chunk::{SectionPos, SECTION_SIZE, SKY_FULL};
use crate::furnace::Facing;
use crate::section::Section;
use crate::torch::{warm_amount, warm_tint};

use super::face::{cactus_quad, cross_quads, quad_for, should_flip, vertex_ao, Face, FACES};
use super::tint::{self, tile_tint};
use super::vertex::{
    pack_vertex, ChunkMesh, ModelVertex, Vertex, UV_MODE_NONE, UV_MODE_SHIFT, UV_MODE_STAIR_NEG_X,
    UV_MODE_STAIR_NEG_Z, UV_MODE_STAIR_POS_X, UV_MODE_STAIR_POS_Z, UV_MODE_STAIR_TOP,
};
use super::water::{self, SideVsWater, WaterSurface};

/// The horizontal cube face a furnace's front points to, for its [`Facing`].
#[inline]
fn facing_face(facing: Facing) -> Face {
    match facing {
        Facing::North => Face::NegZ,
        Facing::South => Face::PosZ,
        Facing::West => Face::NegX,
        Facing::East => Face::PosX,
    }
}

#[inline]
fn opposite_face(face: Face) -> Face {
    match face {
        Face::PosX => Face::NegX,
        Face::NegX => Face::PosX,
        Face::PosY => Face::NegY,
        Face::NegY => Face::PosY,
        Face::PosZ => Face::NegZ,
        Face::NegZ => Face::PosZ,
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LeafMeshMode {
    Detailed,
    Simplified,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct MeshOptions {
    pub leaf_mesh_mode: LeafMeshMode,
}

impl MeshOptions {
    pub const DETAILED: Self = Self {
        leaf_mesh_mode: LeafMeshMode::Detailed,
    };

    pub const FAR_LEAVES: Self = Self {
        leaf_mesh_mode: LeafMeshMode::Simplified,
    };
}

#[cfg(test)]
struct MeshContext {
    neighbour_water: fn(i32, i32, i32) -> u8,
    neighbour_chunk_loaded: fn(i32, i32) -> bool,
}

#[cfg(test)]
impl MeshContext {
    fn standalone() -> Self {
        Self {
            neighbour_water: |_, _, _| 0,
            neighbour_chunk_loaded: |_, _| true,
        }
    }
}

#[cfg(test)]
pub fn build_mesh(
    chunk: &Chunk,
    neighbour_block: impl Fn(i32, i32, i32) -> u8,
    neighbour_biome: impl Fn(i32, i32) -> u8,
    neighbour_light: impl Fn(i32, i32, i32) -> u8,
) -> ChunkMesh {
    let ctx = MeshContext::standalone();
    build_mesh_with_context(
        chunk,
        neighbour_block,
        ctx.neighbour_water,
        neighbour_biome,
        neighbour_light,
        |_, _, _| 0,
        ctx.neighbour_chunk_loaded,
        MeshOptions::DETAILED,
    )
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub fn build_mesh_lods_with_loaded_neighbors(
    chunk: &Chunk,
    neighbour_block: impl Fn(i32, i32, i32) -> u8,
    neighbour_water: impl Fn(i32, i32, i32) -> u8,
    neighbour_biome: impl Fn(i32, i32) -> u8,
    neighbour_light: impl Fn(i32, i32, i32) -> u8,
    neighbour_blocklight: impl Fn(i32, i32, i32) -> u8,
    neighbour_chunk_loaded: impl Fn(i32, i32) -> bool,
) -> ChunkMesh {
    let (ox, oz) = chunk.chunk_origin_world();
    let tints = tint::biome_window(ox, oz, &neighbour_biome);
    let mut mesh = chunk_geometry(
        chunk,
        &neighbour_block,
        &neighbour_water,
        &neighbour_light,
        &neighbour_blocklight,
        &neighbour_chunk_loaded,
        &tints,
        MeshOptions::DETAILED,
    );
    if !chunk.blocks_slice().contains(&Block::OakLeaves.id()) {
        return mesh;
    }
    let far = chunk_geometry(
        chunk,
        &neighbour_block,
        &neighbour_water,
        &neighbour_light,
        &neighbour_blocklight,
        &neighbour_chunk_loaded,
        &tints,
        MeshOptions::FAR_LEAVES,
    );
    if far.opaque_idx.len() < mesh.opaque_idx.len() {
        mesh.far_opaque = far.opaque;
        mesh.far_opaque_idx = far.opaque_idx;
    }
    mesh
}

#[cfg(test)]
pub fn build_mesh_with_options(
    chunk: &Chunk,
    neighbour_block: impl Fn(i32, i32, i32) -> u8,
    neighbour_biome: impl Fn(i32, i32) -> u8,
    neighbour_light: impl Fn(i32, i32, i32) -> u8,
    options: MeshOptions,
) -> ChunkMesh {
    let ctx = MeshContext::standalone();
    build_mesh_with_context(
        chunk,
        neighbour_block,
        ctx.neighbour_water,
        neighbour_biome,
        neighbour_light,
        |_, _, _| 0,
        ctx.neighbour_chunk_loaded,
        options,
    )
}

/// Build the mesh for one cubic [`Section`]. All neighbour lookups are by WORLD
/// coordinate and route to the owning section (including this one), so the same
/// closure handles in-section and cross-section reads; out-of-world / unloaded
/// reads return air / open-sky as the closures define. Block-entity state (furnace
/// lit/facing, torch placement, model offset/facing) is read from `section`
/// directly. The renderer culls the resulting mesh by its [`SectionPos`].
#[allow(clippy::too_many_arguments)]
pub fn build_section_mesh(
    section: &Section,
    pos: SectionPos,
    neighbour_block: impl Fn(i32, i32, i32) -> u8,
    neighbour_water: impl Fn(i32, i32, i32) -> u8,
    neighbour_biome: impl Fn(i32, i32) -> u8,
    neighbour_light: impl Fn(i32, i32, i32) -> u8,
    neighbour_blocklight: impl Fn(i32, i32, i32) -> u8,
    neighbour_loaded: impl Fn(i32, i32, i32) -> bool,
) -> ChunkMesh {
    let tints = section.has_biome_tint_blocks().then(|| {
        let (ox, _, oz) = pos.origin_world();
        tint::biome_window(ox, oz, &neighbour_biome)
    });
    let mut mesh = section_geometry(
        section,
        pos,
        &neighbour_block,
        &neighbour_water,
        &neighbour_light,
        &neighbour_blocklight,
        &neighbour_loaded,
        tints.as_ref(),
        MeshOptions::DETAILED,
    );
    if !section.blocks_slice().contains(&Block::OakLeaves.id()) {
        return mesh;
    }
    let far = section_geometry(
        section,
        pos,
        &neighbour_block,
        &neighbour_water,
        &neighbour_light,
        &neighbour_blocklight,
        &neighbour_loaded,
        tints.as_ref(),
        MeshOptions::FAR_LEAVES,
    );
    if far.opaque_idx.len() < mesh.opaque_idx.len() {
        mesh.far_opaque = far.opaque;
        mesh.far_opaque_idx = far.opaque_idx;
    }
    mesh
}

#[allow(clippy::too_many_arguments)]
fn section_geometry(
    section: &Section,
    pos: SectionPos,
    neighbour_block: impl Fn(i32, i32, i32) -> u8,
    neighbour_water: impl Fn(i32, i32, i32) -> u8,
    neighbour_light: impl Fn(i32, i32, i32) -> u8,
    neighbour_blocklight: impl Fn(i32, i32, i32) -> u8,
    neighbour_loaded: impl Fn(i32, i32, i32) -> bool,
    tints: Option<&tint::BiomeTints>,
    options: MeshOptions,
) -> ChunkMesh {
    let mut opaque = vec![];
    let mut opaque_idx = vec![];
    let mut transparent = vec![];
    let mut transparent_idx = vec![];
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
    let water_at = |wx: i32, wy: i32, wz: i32| -> u8 { neighbour_water(wx, wy, wz) };
    let fluid_at = |wx: i32, wy: i32, wz: i32| -> Option<f32> {
        if block_at(wx, wy, wz) != Block::Water {
            return None;
        }
        let above = block_at(wx, wy + 1, wz) == Block::Water;
        Some(crate::world::water::fluid_height(
            water_at(wx, wy, wz),
            above,
        ))
    };
    let water_fills_cell = |wx: i32, wy: i32, wz: i32| -> bool {
        if block_at(wx, wy, wz) != Block::Water {
            return false;
        }
        let above = block_at(wx, wy + 1, wz) == Block::Water;
        crate::world::water::fills_cell(water_at(wx, wy, wz), above)
    };

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
                if block.render_shape() == RenderShape::Door {
                    continue;
                }

                let wx = ox + lx as i32;
                let wy = oy + ly as i32;
                let wz = oz + lz as i32;
                let ci = lz * SECTION_SIZE + lx;

                if block.render_shape() == RenderShape::Cross {
                    let tile = block.tiles()[0];
                    let l = neighbour_light(wx, wy, wz) as u32;
                    let bl = neighbour_blocklight(wx, wy, wz) as u32;
                    let (sky6, warm) = fold_light(l, bl, SKY_FULL as u32);
                    let tint = warm_tint(tint_tile(tile_tint(tile), ci), warm);
                    emit_cross(
                        &mut opaque,
                        &mut opaque_idx,
                        wx as f32,
                        wy as f32,
                        wz as f32,
                        tile,
                        tint,
                        sky6,
                    );
                    continue;
                }

                if block.render_shape() == RenderShape::Torch {
                    let [top_tile, _bottom, side_tile] = block.tiles();
                    let cell_sky = neighbour_light(wx, wy, wz) as u32;
                    let lit = cell_sky.max(block.light_emission() as u32);
                    let light6 = ((lit * 63 + SKY_FULL as u32 / 2) / SKY_FULL as u32).min(63);
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
                        light6,
                    );
                    continue;
                }

                if let RenderShape::Model(kind) = block.render_shape() {
                    let offset = section.model_offset(lx, ly, lz);
                    let facing = section.model_facing(lx, ly, lz);
                    let l = neighbour_light(wx, wy, wz) as u32;
                    let bl = neighbour_blocklight(wx, wy, wz) as u32;
                    let (light6, warm) = fold_light(l, bl, SKY_FULL as u32);
                    emit_model_block(
                        &mut model,
                        &mut model_idx,
                        kind,
                        offset,
                        facing,
                        wx,
                        wy,
                        wz,
                        light6,
                        warm,
                    );
                    continue;
                }

                if block.render_shape() == RenderShape::Stair {
                    let [tile_top, tile_bot, tile_side] = block.tiles();
                    let tint_for = |tile| tint_tile(tile_tint(tile), ci);
                    emit_stair_block(
                        &mut opaque,
                        &mut opaque_idx,
                        wx,
                        wy,
                        wz,
                        section.stair_facing(lx, ly, lz),
                        [tile_top, tile_bot, tile_side],
                        &tint_for,
                        &block_at,
                        &neighbour_light,
                        &neighbour_blocklight,
                    );
                    continue;
                }

                let is_water = block == Block::Water;
                let [tile_top, tile_bot, tile_side] = block.tiles();
                let furnace_faces = (block == Block::Furnace).then(|| {
                    let front = if section.is_furnace_lit(lx, ly, lz) {
                        Tile::FurnaceFrontOn
                    } else {
                        Tile::FurnaceFront
                    };
                    (facing_face(section.furnace_facing(lx, ly, lz)), front)
                });
                let base_x = wx as f32;
                let base_z = wz as f32;
                let base_y = wy as f32;

                let water_surface = is_water.then(|| {
                    let full = water_fills_cell(wx, wy, wz);
                    WaterSurface::new(wx, wy, wz, full, &block_at, &fluid_at)
                });

                for face in FACES {
                    let (dx, dy, dz) = face.dir();
                    let nwx = wx + dx;
                    let nwy = wy + dy;
                    let nwz = wz + dz;
                    let nb = block_at(nwx, nwy, nwz);

                    let is_water_top = is_water && matches!(face, Face::PosY);
                    let is_side = matches!(face, Face::PosX | Face::NegX | Face::PosZ | Face::NegZ);
                    let is_cactus_side = block == Block::Cactus && is_side;
                    if nb.is_opaque() && !is_water_top && !is_cactus_side {
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
                            Face::NegY => Tile::WaterStill,
                            _ => Tile::WaterFlow,
                        };
                        (t, None, tint_water(ci))
                    } else if block == Block::Grass && is_side {
                        (Tile::Dirt, Some(Tile::GrassSideOverlay), tint_grass(ci))
                    } else {
                        let t = match face {
                            Face::PosY => tile_top,
                            Face::NegY => tile_bot,
                            _ => match furnace_faces {
                                Some((front_face, front_tile)) if face == front_face => front_tile,
                                Some(_) => Tile::FurnaceSide,
                                None => tile_side,
                            },
                        };
                        let tint = tint_tile(tile_tint(t), ci);
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
                        Some(o) => (o as u32, true),
                        None => (water_ov, false),
                    };

                    let (vbuf, ibuf) = if is_water {
                        (&mut transparent, &mut transparent_idx)
                    } else {
                        (&mut opaque, &mut opaque_idx)
                    };

                    let tris = emit_cube_face(
                        vbuf,
                        ibuf,
                        corners,
                        base_tile,
                        overlay,
                        has_overlay,
                        UV_MODE_NONE,
                        tint,
                        face,
                        fx,
                        fy,
                        fz,
                        f_l,
                        f_bl,
                        true,
                        &block_at,
                        &neighbour_light,
                        &neighbour_blocklight,
                    );
                    if is_water && matches!(face, Face::PosY) {
                        ibuf.extend_from_slice(&water::top_back_winding(tris));
                    }
                }
            }
        }
    }

    ChunkMesh {
        opaque,
        opaque_idx,
        transparent,
        transparent_idx,
        far_opaque: vec![],
        far_opaque_idx: vec![],
        model,
        model_idx,
        mesh_dirty: true,
    }
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn build_mesh_with_context(
    chunk: &Chunk,
    neighbour_block: impl Fn(i32, i32, i32) -> u8,
    neighbour_water: impl Fn(i32, i32, i32) -> u8,
    neighbour_biome: impl Fn(i32, i32) -> u8,
    neighbour_light: impl Fn(i32, i32, i32) -> u8,
    neighbour_blocklight: impl Fn(i32, i32, i32) -> u8,
    neighbour_chunk_loaded: impl Fn(i32, i32) -> bool,
    options: MeshOptions,
) -> ChunkMesh {
    let (ox, oz) = chunk.chunk_origin_world();
    let tints = tint::biome_window(ox, oz, &neighbour_biome);
    chunk_geometry(
        chunk,
        neighbour_block,
        neighbour_water,
        neighbour_light,
        neighbour_blocklight,
        neighbour_chunk_loaded,
        &tints,
        options,
    )
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn chunk_geometry(
    chunk: &Chunk,
    neighbour_block: impl Fn(i32, i32, i32) -> u8,
    neighbour_water: impl Fn(i32, i32, i32) -> u8,
    neighbour_light: impl Fn(i32, i32, i32) -> u8,
    neighbour_blocklight: impl Fn(i32, i32, i32) -> u8,
    neighbour_chunk_loaded: impl Fn(i32, i32) -> bool,
    tints: &tint::BiomeTints,
    options: MeshOptions,
) -> ChunkMesh {
    let mut opaque = vec![];
    let mut opaque_idx = vec![];
    let mut transparent = vec![];
    let mut transparent_idx = vec![];
    // bbmodel-block geometry (explicit-UV, model atlas), drawn in the dedicated model
    // pass. Empty for the common chunk.
    let mut model: Vec<ModelVertex> = vec![];
    let mut model_idx: Vec<u32> = vec![];

    let (ox, oz) = chunk.chunk_origin_world();

    // The block at world coords, for AO/light neighbourhood sampling. Mirrors the
    // face-cull bounds logic: read this chunk directly when the column is
    // in-bounds, else defer to the neighbour lookup. Out-of-range Y and missing
    // neighbours read as air, so AO fades to fully-lit at the world's vertical
    // edges and at unloaded chunk borders. Callers pick `occludes_ao()` (AO, incl.
    // leaves) vs `is_opaque()` (which cells carry light) as needed.
    let block_at = |wx: i32, wy: i32, wz: i32| -> Block {
        if wy < 0 || wy >= CHUNK_SY as i32 {
            return Block::Air;
        }
        let lx = wx - ox;
        let lz = wz - oz;
        let id = if lx >= 0 && lx < CHUNK_SX as i32 && lz >= 0 && lz < CHUNK_SZ as i32 {
            chunk.block_raw(lx as usize, wy as usize, lz as usize)
        } else {
            neighbour_block(wx, wy, wz)
        };
        Block::from_id(id)
    };

    // Flowing-water metadata at a world voxel: this chunk in-bounds, else the
    // neighbour lookup (0 = source/none).
    let water_at = |wx: i32, wy: i32, wz: i32| -> u8 {
        if wy < 0 || wy >= CHUNK_SY as i32 {
            return 0;
        }
        let lx = wx - ox;
        let lz = wz - oz;
        if lx >= 0 && lx < CHUNK_SX as i32 && lz >= 0 && lz < CHUNK_SZ as i32 {
            chunk.water_meta(lx as usize, wy as usize, lz as usize)
        } else {
            neighbour_water(wx, wy, wz)
        }
    };
    // Fluid surface height (0..1) of the water cell at a world voxel, or `None`
    // if it is not water. Water with water directly above fills to the top.
    let fluid_at = |wx: i32, wy: i32, wz: i32| -> Option<f32> {
        if block_at(wx, wy, wz) != Block::Water {
            return None;
        }
        let above = block_at(wx, wy + 1, wz) == Block::Water;
        Some(crate::world::water::fluid_height(
            water_at(wx, wy, wz),
            above,
        ))
    };
    let water_fills_cell = |wx: i32, wy: i32, wz: i32| -> bool {
        if block_at(wx, wy, wz) != Block::Water {
            return false;
        }
        let above = block_at(wx, wy + 1, wz) == Block::Water;
        crate::world::water::fills_cell(water_at(wx, wy, wz), above)
    };

    // Skip the all-air shell above the terrain. `heightmap[i]` is the highest
    // non-air Y in column i (set for every non-air block incl. water; rebuilt by
    // recompute_heightmap when block data arrives raw -- see worker.rs). Bounding
    // the outer loop by the chunk-wide max is byte-identical to looping 0..CHUNK_SY:
    // every skipped iteration (y > max_h) has an air centre voxel that would hit
    // the `Block::Air { continue }` guard below and emit zero bytes. We use the
    // chunk-wide max (NOT a per-column bound) so the y-major emission order -- and
    // thus the alpha-blended transparent buffer ordering -- is exactly preserved.
    let max_h = chunk.heightmap.iter().copied().max().unwrap_or(0) as usize;
    for y in 0..=max_h {
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                let id = chunk.block_raw(x, y, z);
                let block = Block::from_id(id);
                if block == Block::Air {
                    continue;
                }

                // Chests are not meshed into the chunk: their inset body + hinged lid
                // are drawn each frame as a dynamic model (see render::chest_model), so
                // the chunk emits nothing here. The block stays SOLID (collision /
                // raycast) but non-opaque, so neighbours keep their faces toward it and
                // there's no hole behind the inset model.
                if block == Block::Chest {
                    continue;
                }

                // Doors are not meshed into the chunk either: each is drawn every frame
                // as a dynamic hinged slab (see render::door_model) so it can swing. The
                // block stays SOLID (collision / raycast) but non-opaque, so neighbours
                // keep their faces toward the thin panel.
                if block.render_shape() == RenderShape::Door {
                    continue;
                }

                // Cross-model plants: two diagonal billboard quads in the opaque
                // (cutout) pass, then skip the cube face loop. They never cull or
                // get culled (non-opaque), carry no directional shade or AO, and
                // sample their own cell's skylight.
                if block.render_shape() == RenderShape::Cross {
                    let ci = z * CHUNK_SX + x;
                    let tile = block.tiles()[0];
                    let wx = ox + x as i32;
                    let wz = oz + z as i32;
                    // Fold skylight + torch block-light: a flower near a torch
                    // brightens and takes the same warm tint as the blocks around it.
                    let l = neighbour_light(wx, y as i32, wz) as u32;
                    let bl = neighbour_blocklight(wx, y as i32, wz) as u32;
                    let (sky6, warm) = fold_light(l, bl, SKY_FULL as u32);
                    let tint = warm_tint(tints.tile(tile_tint(tile), ci), warm);
                    emit_cross(
                        &mut opaque,
                        &mut opaque_idx,
                        wx as f32,
                        y as f32,
                        wz as f32,
                        tile,
                        tint,
                        sky6,
                    );
                    continue;
                }

                // Torch: a thin 3D pole (floor or wall-mounted), baked into the
                // opaque pass with its orientation read from this chunk's torch map.
                // Self-lit to at least its own emission so it stays visible/glowing
                // even where skylight is 0; the surrounding warm glow is the
                // block-light flood sampled by the cube faces below.
                if block.render_shape() == RenderShape::Torch {
                    let [top_tile, _bottom, side_tile] = block.tiles();
                    let wx = ox + x as i32;
                    let wz = oz + z as i32;
                    let cell_sky = neighbour_light(wx, y as i32, wz) as u32;
                    let lit = cell_sky.max(block.light_emission() as u32);
                    let light6 = ((lit * 63 + SKY_FULL as u32 / 2) / SKY_FULL as u32).min(63);
                    let placement = chunk.torch_placement(x, y, z);
                    super::torch::emit_torch(
                        &mut opaque,
                        &mut opaque_idx,
                        wx as f32,
                        y as f32,
                        wz as f32,
                        placement,
                        side_tile,
                        top_tile,
                        [1.0, 1.0, 1.0],
                        light6,
                    );
                    continue;
                }

                // bbmodel blocks: NOT packed into the legacy mesh. Their geometry rides
                // the explicit-UV `model` stream (own texture/atlas), but they're still
                // chunk-meshed here — baked once per remesh and lit at mesh time exactly
                // like any block. A multi-block cell renders only ITS footprint cubes
                // (the split by `model_offset`); a missing-neighbour face isn't culled
                // (the block is non-opaque), so it reads as a placed object, not a cube.
                if let RenderShape::Model(kind) = block.render_shape() {
                    let wx = ox + x as i32;
                    let wz = oz + z as i32;
                    let offset = chunk.model_offset(x, y, z);
                    let facing = chunk.model_facing(x, y, z);
                    let l = neighbour_light(wx, y as i32, wz) as u32;
                    let bl = neighbour_blocklight(wx, y as i32, wz) as u32;
                    let (light6, warm) = fold_light(l, bl, SKY_FULL as u32);
                    emit_model_block(
                        &mut model,
                        &mut model_idx,
                        kind,
                        offset,
                        facing,
                        wx,
                        y as i32,
                        wz,
                        light6,
                        warm,
                    );
                    continue;
                }

                if block.render_shape() == RenderShape::Stair {
                    let [tile_top, tile_bot, tile_side] = block.tiles();
                    let wx = ox + x as i32;
                    let wz = oz + z as i32;
                    let ci = z * CHUNK_SX + x;
                    let tint_for = |tile| tints.tile(tile_tint(tile), ci);
                    emit_stair_block(
                        &mut opaque,
                        &mut opaque_idx,
                        wx,
                        y as i32,
                        wz,
                        crate::block_model::DEFAULT_MODEL_FACING,
                        [tile_top, tile_bot, tile_side],
                        &tint_for,
                        &block_at,
                        &neighbour_light,
                        &neighbour_blocklight,
                    );
                    continue;
                }

                // Only water is alpha-blended; leaves render in the OPAQUE pass
                // (crisp/cutout, no see-through ghosting) per the "fully opaque" rule.
                let is_water = block == Block::Water;

                // Choose tile for each face.
                let [tile_top, tile_bot, tile_side] = block.tiles();
                // A furnace shows its front (lit/unlit) only on the face it was
                // placed facing; the other three horizontal faces use furnace_side.
                // Read from this chunk's own furnace map (in-chunk only — facing
                // never affects cross-border culling).
                let furnace_faces = (block == Block::Furnace).then(|| {
                    let front = if chunk.is_furnace_lit(x, y, z) {
                        Tile::FurnaceFrontOn
                    } else {
                        Tile::FurnaceFront
                    };
                    (facing_face(chunk.furnace_facing(x, y, z)), front)
                });
                let ci = z * CHUNK_SX + x;
                let base_x = x as f32 + ox as f32;
                let base_z = z as f32 + oz as f32;

                // Water surface geometry (corner heights, flow tile/heading, full-
                // height flag), computed once per cell — see `mesh::water`. `None`
                // for non-water blocks.
                let water_surface = is_water.then(|| {
                    let full = water_fills_cell(ox + x as i32, y as i32, oz + z as i32);
                    WaterSurface::new(
                        ox + x as i32,
                        y as i32,
                        oz + z as i32,
                        full,
                        &block_at,
                        &fluid_at,
                    )
                });

                for face in FACES {
                    let (dx, dy, dz) = face.dir();
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    let nz = z as i32 + dz;

                    // Neighbour block to test cull.
                    let mut unloaded_horizontal_neighbor = false;
                    let nb_id =
                        if nx < 0 || nx >= CHUNK_SX as i32 || nz < 0 || nz >= CHUNK_SZ as i32 {
                            // Out of horizontal chunk bounds -> ask neighbour fn.
                            let wx = ox + nx;
                            let wz = oz + nz;
                            unloaded_horizontal_neighbor =
                                !neighbour_chunk_loaded(wx >> 4, wz >> 4);
                            if ny < 0 || ny >= CHUNK_SY as i32 {
                                0 // air
                            } else {
                                neighbour_block(wx, ny, wz)
                            }
                        } else if ny < 0 || ny >= CHUNK_SY as i32 {
                            0
                        } else {
                            chunk.block_raw(nx as usize, ny as usize, nz as usize)
                        };
                    let nb = Block::from_id(nb_id);

                    // Cull rule: a face is hidden only if the neighbour is a full
                    // opaque cube (`is_opaque()` -- stone/dirt/grass/sand/snow/log).
                    // Leaves are NOT opaque-for-culling (they're a cutout), so
                    // leaf<->leaf faces are intentionally NOT culled -- every leaf
                    // cube draws all its faces, giving a dense canopy you can't see
                    // through to the sky. Water additionally culls against itself.
                    //
                    // Exception: water's TOP face is kept even under an opaque
                    // block. The surface sits recessed below the block, so culling
                    // it punches a hole (the floor shows through where a block caps
                    // the water). Water's bottom/side faces against opaque still cull.
                    let is_water_top = is_water && matches!(face, Face::PosY);
                    let is_side = matches!(face, Face::PosX | Face::NegX | Face::PosZ | Face::NegZ);
                    // A cactus's four sides are inset 1px (see `cactus_quad`), so they stay
                    // visible in the gap even when a solid block sits flush against the cell
                    // — never cull them. Its flush top/bottom DO cull normally, so the
                    // bottom hides against the (opaque) block the cactus rests on.
                    let is_cactus_side = block == Block::Cactus && is_side;
                    if nb.is_opaque() && !is_water_top && !is_cactus_side {
                        continue;
                    }
                    if is_water && is_side && unloaded_horizontal_neighbor {
                        continue;
                    }
                    if options.leaf_mesh_mode == LeafMeshMode::Simplified
                        && block == Block::OakLeaves
                        && nb == Block::OakLeaves
                    {
                        continue;
                    }
                    // Set on the one water-water side face we DON'T cull: a
                    // full-height cell's exposed step over a shorter neighbour.
                    // Its bottom is trimmed to the neighbour's surface (below) so
                    // the submerged part — water behind water — isn't drawn twice.
                    // Otherwise faces between two water cells cull (the surfaces meet).
                    let mut water_exposed_step = false;
                    if let Some(ws) = &water_surface {
                        if nb == Block::Water {
                            let nb_full = water_fills_cell(ox + nx, y as i32, oz + nz);
                            match ws.side_against_water(is_side, nb_full) {
                                SideVsWater::ExposedStep => water_exposed_step = true,
                                SideVsWater::Cull => continue,
                            }
                        }
                    }

                    // Material for this face: base tile + optional biome-tinted
                    // overlay + tint + texture rotation. Water tops use the still
                    // or flow tile (rotated toward the flow); water sides always
                    // use the downward-flowing tile. Grass block SIDES render as
                    // dirt + a grayscale grass overlay tinted by the same biome
                    // grass colour as the top. Everything else is the face's own
                    // tile, tinted only for grass-top/foliage/water.
                    let (base_tile, overlay_tile, tint) = if let Some(ws) = &water_surface {
                        let t = match face {
                            Face::PosY => ws.top_tile(),
                            Face::NegY => Tile::WaterStill,
                            _ => Tile::WaterFlow,
                        };
                        (t, None, tints.water[ci])
                    } else if block == Block::Grass && is_side {
                        (Tile::Dirt, Some(Tile::GrassSideOverlay), tints.grass[ci])
                    } else {
                        let t = match face {
                            Face::PosY => tile_top,
                            Face::NegY => tile_bot,
                            _ => match furnace_faces {
                                Some((front_face, front_tile)) if face == front_face => front_tile,
                                Some(_) => Tile::FurnaceSide,
                                None => tile_side,
                            },
                        };
                        let tint = tints.tile(tile_tint(t), ci);
                        (t, None, tint)
                    };

                    // Build quad vertices in CCW order when viewed from outside.
                    // Positions are in world space (baked chunk origin) so each
                    // chunk renders at its actual world coordinates.
                    let base_y = y as f32;
                    // The cactus recesses its four side faces 1/16 so its spines show; every
                    // other cube fills the cell. Top/bottom stay full (a flush stack of
                    // segments; the cap overhangs the trunk).
                    let mut corners = if block == Block::Cactus {
                        cactus_quad(
                            face,
                            [base_x, base_y, base_z],
                            [base_x + 1.0, base_y + 1.0, base_z + 1.0],
                        )
                    } else {
                        quad_for(face, base_x, base_y, base_z)
                    };

                    // Water vertices are warped onto the cell's surface (top edge to
                    // the per-corner height; exposed-step faces also trim the bottom).
                    if let Some(ws) = &water_surface {
                        ws.warp_quad(&mut corners, base_x, base_y, base_z, water_exposed_step);
                    }

                    // The front voxel F = block + normal, and its pre-sampled light:
                    // the shared seed for every corner's AO + smooth-skylight sample.
                    let fx = ox + x as i32 + dx;
                    let fy = y as i32 + dy;
                    let fz = oz + z as i32 + dz;
                    let f_l = neighbour_light(fx, fy, fz) as u32;
                    let f_bl = neighbour_blocklight(fx, fy, fz) as u32;

                    // Resolve this face's 12..20 overlay payload + has-overlay flag.
                    // A grass SIDE carries the tinted GrassSideOverlay; a flowing
                    // water TOP (no grass overlay) reuses those 8 bits to carry the
                    // quantized flow heading with the flag CLEAR, so the fragment
                    // shader composites no overlay (water side faces derive their
                    // texture V from the vertex height in the shader, so they need
                    // no per-face data here). `pack_vertex` owns the bit positions.
                    let water_ov: u32 = match &water_surface {
                        Some(ws) if matches!(face, Face::PosY) => ws.top_angle(),
                        _ => 0,
                    };
                    let (overlay, has_overlay) = match overlay_tile {
                        Some(o) => (o as u32, true),
                        None => (water_ov, false),
                    };

                    let (vbuf, ibuf) = if is_water {
                        (&mut transparent, &mut transparent_idx)
                    } else {
                        (&mut opaque, &mut opaque_idx)
                    };

                    let tris = emit_cube_face(
                        vbuf,
                        ibuf,
                        corners,
                        base_tile,
                        overlay,
                        has_overlay,
                        UV_MODE_NONE,
                        tint,
                        face,
                        fx,
                        fy,
                        fz,
                        f_l,
                        f_bl,
                        true,
                        &block_at,
                        &neighbour_light,
                        &neighbour_blocklight,
                    );
                    // A water surface (top face) also emits its reverse winding so it
                    // stays visible from underneath in the back-face-culled
                    // transparent pass; side/bottom faces stay single-sided.
                    if is_water && matches!(face, Face::PosY) {
                        ibuf.extend_from_slice(&water::top_back_winding(tris));
                    }
                }
            }
        }
    }

    ChunkMesh {
        opaque,
        opaque_idx,
        transparent,
        transparent_idx,
        far_opaque: vec![],
        far_opaque_idx: vec![],
        model,
        model_idx,
        mesh_dirty: true,
    }
}

/// Mesh-time brightness for a bbmodel-block face from the cell's combined 6-bit light.
/// Mirrors `block.wgsl`'s skylight curve (the block pipeline applies this in the shader;
/// the model pass shader just multiplies, so we bake the curve in here) — keep the
/// constants in sync.
#[inline]
fn model_light_factor(light6: u32) -> f32 {
    const SKY_MIN: f32 = 0.02;
    const FINAL_MIN: f32 = 0.006;
    let s = (light6 as f32 / 63.0).clamp(0.0, 1.0);
    (SKY_MIN + (1.0 - SKY_MIN) * s * s * s).max(FINAL_MIN)
}

/// Stream one bbmodel-block cell's geometry into the `model` buffers: copy the cell's
/// startup-baked template (positions already taken through the cube rotation + placement
/// facing) translated to the world base, folding the cell's combined light into each
/// vertex's directional shade and applying the warm block-light tint. No matrices /
/// quaternions / face-bias work happens per remesh — it's all resolved once in
/// [`block_model::ModelInstance`], so meshing a placed model is a translate + scale + copy.
#[allow(clippy::too_many_arguments)]
fn emit_model_block(
    verts: &mut Vec<ModelVertex>,
    indices: &mut Vec<u32>,
    kind: BlockModelKind,
    offset: [u8; 3],
    facing: Facing,
    wx: i32,
    wy: i32,
    wz: i32,
    light6: u32,
    warm: f32,
) {
    let inst = block_model::instance(kind);
    let Some(tmpl) = inst.cell_template(offset, facing) else {
        return;
    };
    // The chunk stores the authored cell offset + placed facing; together those resolve the
    // rotated footprint base. The template's vertices are baked relative to that base, so
    // placing the cell is one translate per vertex.
    let base = block_model::base_from_cell(IVec3::new(wx, wy, wz), kind, offset, facing);
    let basef = Vec3::new(base.x as f32, base.y as f32, base.z as f32);
    let light = model_light_factor(light6);
    let tint = warm_tint([1.0, 1.0, 1.0], warm);
    let start = verts.len() as u32;
    verts.extend(tmpl.verts.iter().map(|v| ModelVertex {
        pos: (basef + v.pos).to_array(),
        uv: v.uv,
        shade: v.shade * light,
        tint,
    }));
    indices.extend(tmpl.indices.iter().map(|&i| start + i));
}

#[allow(clippy::too_many_arguments)]
fn emit_stair_block<B, L, K, T>(
    opaque: &mut Vec<Vertex>,
    opaque_idx: &mut Vec<u32>,
    wx: i32,
    wy: i32,
    wz: i32,
    facing: Facing,
    tiles: [Tile; 3],
    tint_for: &T,
    block_at: &B,
    neighbour_light: &L,
    neighbour_blocklight: &K,
) where
    B: Fn(i32, i32, i32) -> Block,
    L: Fn(i32, i32, i32) -> u8,
    K: Fn(i32, i32, i32) -> u8,
    T: Fn(Tile) -> [f32; 3],
{
    let front = facing_face(facing);
    let internal_low_back = opposite_face(front);
    let boxes = crate::stair::boxes(facing);
    let low = boxes[0];
    let high = boxes[1];

    for face in FACES {
        if face != internal_low_back {
            emit_stair_face(
                opaque,
                opaque_idx,
                wx,
                wy,
                wz,
                low.min,
                low.max,
                face,
                tiles,
                tint_for,
                block_at,
                neighbour_light,
                neighbour_blocklight,
            );
        }

        let mut min = high.min;
        let max = high.max;
        if face == front {
            min[1] = low.max[1];
        }
        emit_stair_face(
            opaque,
            opaque_idx,
            wx,
            wy,
            wz,
            min,
            max,
            face,
            tiles,
            tint_for,
            block_at,
            neighbour_light,
            neighbour_blocklight,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_stair_face<B, L, K, T>(
    opaque: &mut Vec<Vertex>,
    opaque_idx: &mut Vec<u32>,
    wx: i32,
    wy: i32,
    wz: i32,
    min: [f32; 3],
    max: [f32; 3],
    face: Face,
    tiles: [Tile; 3],
    tint_for: &T,
    block_at: &B,
    neighbour_light: &L,
    neighbour_blocklight: &K,
) where
    B: Fn(i32, i32, i32) -> Block,
    L: Fn(i32, i32, i32) -> u8,
    K: Fn(i32, i32, i32) -> u8,
    T: Fn(Tile) -> [f32; 3],
{
    if min[0] >= max[0] || min[1] >= max[1] || min[2] >= max[2] {
        return;
    }
    let boundary = face_on_cell_boundary(face, min, max);
    let (dx, dy, dz) = face.dir();
    let (fx, fy, fz) = if boundary {
        (wx + dx, wy + dy, wz + dz)
    } else {
        (wx, wy, wz)
    };
    if boundary && block_at(fx, fy, fz).is_opaque() {
        return;
    }

    let tile = match face {
        Face::PosY => tiles[0],
        Face::NegY => tiles[1],
        _ => tiles[2],
    };
    let uv_mode = stair_uv_mode(face);
    let world_min = [wx as f32 + min[0], wy as f32 + min[1], wz as f32 + min[2]];
    let world_max = [wx as f32 + max[0], wy as f32 + max[1], wz as f32 + max[2]];
    let corners = face.quad_box(world_min, world_max);
    // The underside is a closed face: if the cell below is dark, adjacent sky-lit
    // cells must not smooth light onto it.
    let smooth_light = face != Face::NegY;
    emit_cube_face(
        opaque,
        opaque_idx,
        corners,
        tile,
        0,
        false,
        uv_mode,
        tint_for(tile),
        face,
        fx,
        fy,
        fz,
        neighbour_light(fx, fy, fz) as u32,
        neighbour_blocklight(fx, fy, fz) as u32,
        smooth_light,
        block_at,
        neighbour_light,
        neighbour_blocklight,
    );
}

#[inline]
fn stair_uv_mode(face: Face) -> u32 {
    match face {
        Face::PosX => UV_MODE_STAIR_POS_X,
        Face::NegX => UV_MODE_STAIR_NEG_X,
        Face::PosZ => UV_MODE_STAIR_POS_Z,
        Face::NegZ => UV_MODE_STAIR_NEG_Z,
        Face::PosY => UV_MODE_STAIR_TOP,
        Face::NegY => UV_MODE_NONE,
    }
}

#[inline]
fn face_on_cell_boundary(face: Face, min: [f32; 3], max: [f32; 3]) -> bool {
    const EPS: f32 = 1.0e-6;
    match face {
        Face::PosX => (max[0] - 1.0).abs() <= EPS,
        Face::NegX => min[0].abs() <= EPS,
        Face::PosY => (max[1] - 1.0).abs() <= EPS,
        Face::NegY => min[1].abs() <= EPS,
        Face::PosZ => (max[2] - 1.0).abs() <= EPS,
        Face::NegZ => min[2].abs() <= EPS,
    }
}

/// Fold a cell's (or neighbourhood-summed) skylight + block-light into the packed
/// 6-bit brightness and a 0..1 warm amount. `sum_sky`/`sum_block` are x2-scale
/// sums over `denom = cnt * SKY_FULL` cells (`cnt = 1`, `denom = SKY_FULL` for a
/// single cell). Brightness is the BRIGHTER channel (so a torch lights a sky-dark
/// cave); warm comes from the shared [`warm_amount`](crate::torch::warm_amount) so
/// static blocks and dynamic geometry warm identically.
#[inline]
fn fold_light(sum_sky: u32, sum_block: u32, denom: u32) -> (u32, f32) {
    let light6 = ((sum_sky.max(sum_block) * 63 + denom / 2) / denom).min(63);
    let warm = warm_amount(
        sum_sky as f32 / denom as f32,
        sum_block as f32 / denom as f32,
    );
    (light6, warm)
}

/// One corner's ambient-occlusion level and smooth skylight, sampled from the
/// shared 2x2 neighbourhood just outside the face: the front voxel `F = (fx,fy,fz)`
/// (= block + normal, with its pre-sampled light `f_l`) plus its two edge
/// neighbours and the diagonal one, picked by this `corner`'s `ao_signs`. AO counts
/// solid occluders (opaque cubes AND leaves, for canopy self-occlusion); skylight
/// averages the light of the non-opaque cells of that 2x2 (F is always non-opaque
/// for an emitted face). Returns `(ao 0..3, skylight 0..63)`.
#[allow(clippy::too_many_arguments)]
#[inline]
fn vertex_ao_and_light<B, L, K>(
    face: Face,
    corner: usize,
    fx: i32,
    fy: i32,
    fz: i32,
    f_l: u32,
    f_bl: u32,
    smooth_light: bool,
    block_at: &B,
    neighbour_light: &L,
    neighbour_blocklight: &K,
) -> (u32, u32, f32)
where
    B: Fn(i32, i32, i32) -> Block,
    L: Fn(i32, i32, i32) -> u8,
    K: Fn(i32, i32, i32) -> u8,
{
    let (ux, uy, uz) = face.ao_u();
    let (vx, vy, vz) = face.ao_v();
    let (su, sv) = face.ao_signs()[corner];
    let (e1x, e1y, e1z) = (fx + su * ux, fy + su * uy, fz + su * uz);
    let (e2x, e2y, e2z) = (fx + sv * vx, fy + sv * vy, fz + sv * vz);
    let (dxx, dyy, dzz) = (
        fx + su * ux + sv * vx,
        fy + su * uy + sv * vy,
        fz + su * uz + sv * vz,
    );
    let b1 = block_at(e1x, e1y, e1z);
    let b2 = block_at(e2x, e2y, e2z);
    let bd = block_at(dxx, dyy, dzz);
    let ao = vertex_ao(b1.occludes_ao(), b2.occludes_ao(), bd.occludes_ao());
    if !smooth_light {
        let (light6, warm) = fold_light(f_l, f_bl, SKY_FULL as u32);
        return (ao, light6, warm);
    }

    // Smooth skylight AND block-light: mean over F + the surround cells that carry
    // light (anything not fully opaque -- leaves included, since they transmit
    // light even though they occlude AO). The two channels share the same cells and
    // count, so `fold_light` can max the sums for brightness and compare them for
    // the warm tint.
    let mut sum = f_l;
    let mut sum_block = f_bl;
    let mut cnt = 1u32;
    if !b1.is_opaque() {
        sum += neighbour_light(e1x, e1y, e1z) as u32;
        sum_block += neighbour_blocklight(e1x, e1y, e1z) as u32;
        cnt += 1;
    }
    if !b2.is_opaque() {
        sum += neighbour_light(e2x, e2y, e2z) as u32;
        sum_block += neighbour_blocklight(e2x, e2y, e2z) as u32;
        cnt += 1;
    }
    if !bd.is_opaque() {
        sum += neighbour_light(dxx, dyy, dzz) as u32;
        sum_block += neighbour_blocklight(dxx, dyy, dzz) as u32;
        cnt += 1;
    }
    // avg in [0,SKY_FULL] -> 6-bit level in [0,63], integer round-half-up (no f32
    // for the level, to keep skylight-only meshes byte-identical).
    let denom = cnt * SKY_FULL as u32;
    let (light6, warm) = fold_light(sum, sum_block, denom);
    (ao, light6, warm)
}

/// Emit one resolved cube face: sample per-corner AO + skylight from the shared 2x2
/// neighbourhood, push the four packed vertices, and append the (AO-symmetric,
/// possibly flipped) triangulation. Returns the two triangles' six indices so the
/// caller can add water's reverse winding before closing the section. The `corners`
/// are already in world space (and water-warped); `face` drives shade + the AO
/// neighbourhood; `(fx,fy,fz)`/`f_l` are the front voxel and its light.
#[allow(clippy::too_many_arguments)]
fn emit_cube_face<B, L, K>(
    vbuf: &mut Vec<Vertex>,
    ibuf: &mut Vec<u32>,
    corners: [[f32; 3]; 4],
    base_tile: Tile,
    overlay: u32,
    has_overlay: bool,
    uv_mode: u32,
    tint: [f32; 3],
    face: Face,
    fx: i32,
    fy: i32,
    fz: i32,
    f_l: u32,
    f_bl: u32,
    smooth_light: bool,
    block_at: &B,
    neighbour_light: &L,
    neighbour_blocklight: &K,
) -> [u32; 6]
where
    B: Fn(i32, i32, i32) -> Block,
    L: Fn(i32, i32, i32) -> u8,
    K: Fn(i32, i32, i32) -> u8,
{
    let shade_idx = face.shade_idx();
    let mut ao = [3u32; 4];
    let start = vbuf.len() as u32;
    for (corner, p) in corners.into_iter().enumerate() {
        let (a, light6, warm) = vertex_ao_and_light(
            face,
            corner,
            fx,
            fy,
            fz,
            f_l,
            f_bl,
            smooth_light,
            block_at,
            neighbour_light,
            neighbour_blocklight,
        );
        ao[corner] = a;
        vbuf.push(Vertex {
            pos: p,
            // Warm the face tint per corner by however much torch light reaches it,
            // so the glow fades smoothly across the surface (0 warm = unchanged).
            tint: warm_tint(tint, warm),
            packed: pack_vertex(
                base_tile as u32,
                corner as u32,
                shade_idx,
                overlay,
                has_overlay,
                a,
                light6,
            ) | (uv_mode << UV_MODE_SHIFT),
        });
    }
    // Flip the triangulation so the split runs along the darker diagonal -- keeps
    // the AO gradient symmetric (no bright bleed).
    let tris: [u32; 6] = if should_flip(ao) {
        [start, start + 1, start + 3, start + 1, start + 2, start + 3]
    } else {
        [start, start + 1, start + 2, start, start + 2, start + 3]
    };
    ibuf.extend_from_slice(&tris);
    tris
}

/// Emit an X-shaped plant: two diagonal billboard quads into the opaque (cutout)
/// buffer, each drawn in BOTH windings so the plant is visible from both sides
/// under back-face culling. Flat-lit (AO = 3, shade index 0 = "top", no
/// directional darkening), biome-tinted for grass/fern; `fs_opaque`'s alpha
/// discard handles the transparent texels exactly like leaves.
fn emit_cross(
    opaque: &mut Vec<Vertex>,
    opaque_idx: &mut Vec<u32>,
    bx: f32,
    y: f32,
    bz: f32,
    tile: Tile,
    tint: [f32; 3],
    sky6: u32,
) {
    // Flat-lit: shade index 0 (top, no directional darkening), AO = 3, no overlay;
    // `pack_vertex` owns the bit layout.
    for plane in cross_quads(bx, y, bz) {
        let start = opaque.len() as u32;
        for (corner, p) in plane.into_iter().enumerate() {
            opaque.push(Vertex {
                pos: p,
                tint,
                packed: pack_vertex(tile as u32, corner as u32, 0, 0, false, 3, sky6),
            });
        }
        opaque_idx.extend_from_slice(&[start, start + 1, start + 2, start, start + 2, start + 3]);
        opaque_idx.extend_from_slice(&[start, start + 2, start + 1, start, start + 3, start + 2]);
    }
}
