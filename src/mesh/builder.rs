use crate::atlas::Tile;
use crate::block::{Block, RenderShape};
use crate::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SECTION_COUNT, SECTION_SIZE, SKY_FULL};
use crate::furnace::Facing;
use crate::torch::{warm_amount, warm_tint};

use super::face::{cross_quads, quad_for, should_flip, vertex_ao, Face, FACES};
use super::tint::{self, tile_tint};
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
use super::vertex::{pack_vertex, ChunkMesh, MeshIndexSection, Vertex};

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

/// The two cross-chunk lookups that `build_mesh_with_context` needs beyond the
/// block/biome/light triple, bundled so the "no neighbours" defaults live in one
/// place instead of being re-spelled as stub closures at each entry point.
struct MeshContext {
    /// Flowing-water metadata at a world voxel (0 = source/none).
    neighbour_water: fn(i32, i32, i32) -> u8,
    /// Whether the chunk owning a world column is loaded (gates water edge culling).
    neighbour_chunk_loaded: fn(i32, i32) -> bool,
}

impl MeshContext {
    /// Defaults for meshing a chunk in isolation: no flowing-water metadata across
    /// borders (everything reads as a source), and every neighbour treated as
    /// loaded. Used by `build_mesh` (and the test-only entry points).
    fn standalone() -> Self {
        Self {
            neighbour_water: |_, _, _| 0,
            neighbour_chunk_loaded: |_, _| true,
        }
    }
}

/// Build the mesh for one chunk. Neighbour chunk block lookups are needed for
/// cross-chunk face culling: pass them via `neighbour_block`.
/// `neighbour_biome(wx, wz)` returns biome id at world column; used for
/// biome-blend tints (grass top / water / leaves). `neighbour_light(wx, wy, wz)`
/// returns the cached skylight (x2 scale) at a world voxel -- routed to the owning
/// chunk's stored band -- so meshing just SAMPLES light, never recomputes it.
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

/// `neighbour_water(wx, wy, wz)` returns the flowing-water metadata byte at a
/// world voxel (0 = source/none), routed to the owning chunk just like
/// `neighbour_block`, so water surface heights and flow direction read correctly
/// across chunk borders.
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
    let mut mesh = build_mesh_with_context(
        chunk,
        &neighbour_block,
        &neighbour_water,
        &neighbour_biome,
        &neighbour_light,
        &neighbour_blocklight,
        &neighbour_chunk_loaded,
        MeshOptions::DETAILED,
    );
    if !chunk.blocks_slice().contains(&Block::OakLeaves.id()) {
        return mesh;
    }
    let far = build_mesh_with_context(
        chunk,
        &neighbour_block,
        &neighbour_water,
        &neighbour_biome,
        &neighbour_light,
        &neighbour_blocklight,
        &neighbour_chunk_loaded,
        MeshOptions::FAR_LEAVES,
    );
    if far.opaque_idx.len() < mesh.opaque_idx.len() {
        mesh.far_opaque = far.opaque;
        mesh.far_opaque_idx = far.opaque_idx;
        mesh.far_opaque_sections = far.opaque_sections;
    }
    mesh
}

/// Standalone mesh with explicit [`MeshOptions`] (e.g. the far-leaf LOD), for the
/// LOD/leaf tests. Production reaches the options via
/// [`build_mesh_lods_with_loaded_neighbors`].
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
    let mut opaque = vec![];
    let mut opaque_idx = vec![];
    let mut opaque_sections = [MeshIndexSection::default(); SECTION_COUNT];
    let mut transparent = vec![];
    let mut transparent_idx = vec![];
    let mut transparent_sections = [MeshIndexSection::default(); SECTION_COUNT];

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

    // Precompute biome-blended tint (5x5 window) per column, per kind.
    let tints = tint::biome_window(ox, oz, &neighbour_biome);

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
                    let first_index = opaque_idx.len() as u32;
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
                    extend_section(
                        &mut opaque_sections,
                        section_for_y(y),
                        first_index,
                        opaque_idx.len() as u32 - first_index,
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
                    let first_index = opaque_idx.len() as u32;
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
                    extend_section(
                        &mut opaque_sections,
                        section_for_y(y),
                        first_index,
                        opaque_idx.len() as u32 - first_index,
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
                    if nb.is_opaque() && !is_water_top {
                        continue;
                    }
                    let is_side = matches!(face, Face::PosX | Face::NegX | Face::PosZ | Face::NegZ);
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
                    let mut corners = quad_for(face, base_x, base_y, base_z);

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

                    let (vbuf, ibuf, sections) = if is_water {
                        (
                            &mut transparent,
                            &mut transparent_idx,
                            &mut transparent_sections,
                        )
                    } else {
                        (&mut opaque, &mut opaque_idx, &mut opaque_sections)
                    };

                    let first_index = ibuf.len() as u32;
                    let tris = emit_cube_face(
                        vbuf,
                        ibuf,
                        corners,
                        base_tile,
                        overlay,
                        has_overlay,
                        tint,
                        face,
                        fx,
                        fy,
                        fz,
                        f_l,
                        f_bl,
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
                    extend_section(
                        sections,
                        section_for_y(y),
                        first_index,
                        ibuf.len() as u32 - first_index,
                    );
                }
            }
        }
    }

    ChunkMesh {
        opaque,
        opaque_idx,
        opaque_sections,
        transparent,
        transparent_idx,
        transparent_sections,
        far_opaque: vec![],
        far_opaque_idx: vec![],
        far_opaque_sections: [MeshIndexSection::default(); SECTION_COUNT],
        mesh_dirty: true,
    }
}

#[inline]
fn section_for_y(y: usize) -> usize {
    (y / SECTION_SIZE).min(SECTION_COUNT - 1)
}

fn extend_section(
    sections: &mut [MeshIndexSection; SECTION_COUNT],
    section: usize,
    first_index: u32,
    index_count: u32,
) {
    if index_count == 0 {
        return;
    }
    let existing = &mut sections[section];
    if existing.index_count == 0 {
        *existing = MeshIndexSection {
            first_index,
            index_count,
        };
        return;
    }

    let first = existing.first_index.min(first_index);
    let end = (existing.first_index + existing.index_count).max(first_index + index_count);
    *existing = MeshIndexSection {
        first_index: first,
        index_count: end - first,
    };
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
    let warm = warm_amount(sum_sky as f32 / denom as f32, sum_block as f32 / denom as f32);
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
    tint: [f32; 3],
    face: Face,
    fx: i32,
    fy: i32,
    fz: i32,
    f_l: u32,
    f_bl: u32,
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
            ),
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
