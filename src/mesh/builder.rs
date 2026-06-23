use crate::atlas::Tile;
use crate::biome::Biome;
use crate::block::{Block, RenderShape};
use crate::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SECTION_COUNT, SECTION_SIZE, SKY_FULL};
use crate::furnace::Facing;

use super::face::{cross_quads, quad_for, should_flip, vertex_ao, Face, FACES};

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
use super::vertex::{ChunkMesh, MeshIndexSection, Vertex};

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

#[derive(Copy, Clone)]
enum TintKind {
    Grass,
    Foliage,
    Water,
}

fn tile_tint(tile: Tile) -> Option<TintKind> {
    match tile {
        Tile::GrassTop => Some(TintKind::Grass),
        Tile::ShortGrass => Some(TintKind::Grass),
        Tile::Fern => Some(TintKind::Grass),
        Tile::Water => Some(TintKind::Water),
        Tile::WaterStill => Some(TintKind::Water),
        Tile::WaterFlow => Some(TintKind::Water),
        Tile::OakLeaves => Some(TintKind::Foliage),
        Tile::AcaciaLeaves => Some(TintKind::Foliage),
        Tile::BirchLeaves => Some(TintKind::Foliage),
        Tile::DarkOakLeaves => Some(TintKind::Foliage),
        Tile::JungleLeaves => Some(TintKind::Foliage),
        Tile::MangroveLeaves => Some(TintKind::Foliage),
        Tile::SpruceLeaves => Some(TintKind::Foliage),
        _ => None,
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
    build_mesh_with_context(
        chunk,
        neighbour_block,
        |_, _, _| 0,
        neighbour_biome,
        neighbour_light,
        |_, _| true,
        MeshOptions::DETAILED,
    )
}

pub fn build_mesh_lods(
    chunk: &Chunk,
    neighbour_block: impl Fn(i32, i32, i32) -> u8,
    neighbour_biome: impl Fn(i32, i32) -> u8,
    neighbour_light: impl Fn(i32, i32, i32) -> u8,
) -> ChunkMesh {
    build_mesh_lods_with_loaded_neighbors(
        chunk,
        neighbour_block,
        |_, _, _| 0,
        neighbour_biome,
        neighbour_light,
        |_, _| true,
    )
}

/// `neighbour_water(wx, wy, wz)` returns the flowing-water metadata byte at a
/// world voxel (0 = source/none), routed to the owning chunk just like
/// `neighbour_block`, so water surface heights and flow direction read correctly
/// across chunk borders.
pub fn build_mesh_lods_with_loaded_neighbors(
    chunk: &Chunk,
    neighbour_block: impl Fn(i32, i32, i32) -> u8,
    neighbour_water: impl Fn(i32, i32, i32) -> u8,
    neighbour_biome: impl Fn(i32, i32) -> u8,
    neighbour_light: impl Fn(i32, i32, i32) -> u8,
    neighbour_chunk_loaded: impl Fn(i32, i32) -> bool,
) -> ChunkMesh {
    let mut mesh = build_mesh_with_context(
        chunk,
        &neighbour_block,
        &neighbour_water,
        &neighbour_biome,
        &neighbour_light,
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

pub fn build_mesh_with_options(
    chunk: &Chunk,
    neighbour_block: impl Fn(i32, i32, i32) -> u8,
    neighbour_biome: impl Fn(i32, i32) -> u8,
    neighbour_light: impl Fn(i32, i32, i32) -> u8,
    options: MeshOptions,
) -> ChunkMesh {
    build_mesh_with_context(
        chunk,
        neighbour_block,
        |_, _, _| 0,
        neighbour_biome,
        neighbour_light,
        |_, _| true,
        options,
    )
}

fn build_mesh_with_context(
    chunk: &Chunk,
    neighbour_block: impl Fn(i32, i32, i32) -> u8,
    neighbour_water: impl Fn(i32, i32, i32) -> u8,
    neighbour_biome: impl Fn(i32, i32) -> u8,
    neighbour_light: impl Fn(i32, i32, i32) -> u8,
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
    const R: i32 = 2;
    let n = (2 * R + 1) as f32 * (2 * R + 1) as f32;
    let mut tint_grass = vec![[0f32; 3]; CHUNK_SX * CHUNK_SZ];
    let mut tint_foliage = vec![[0f32; 3]; CHUNK_SX * CHUNK_SZ];
    let mut tint_water = vec![[0f32; 3]; CHUNK_SX * CHUNK_SZ];
    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            let wx = ox + x as i32;
            let wz = oz + z as i32;
            let mut g = [0f32; 3];
            let mut f = [0f32; 3];
            let mut w = [0f32; 3];
            for dz in -R..=R {
                for dx in -R..=R {
                    let b = Biome::from_id(neighbour_biome(wx + dx, wz + dz));
                    let grass = b.grass_color();
                    let foliage = b.foliage_color();
                    let water = b.water_color();
                    g[0] += grass[0];
                    g[1] += grass[1];
                    g[2] += grass[2];
                    f[0] += foliage[0];
                    f[1] += foliage[1];
                    f[2] += foliage[2];
                    w[0] += water[0];
                    w[1] += water[1];
                    w[2] += water[2];
                }
            }
            let i = z * CHUNK_SX + x;
            tint_grass[i] = [g[0] / n, g[1] / n, g[2] / n];
            tint_foliage[i] = [f[0] / n, f[1] / n, f[2] / n];
            tint_water[i] = [w[0] / n, w[1] / n, w[2] / n];
        }
    }

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
                    let tint = match tile_tint(tile) {
                        Some(TintKind::Grass) => tint_grass[ci],
                        Some(TintKind::Foliage) => tint_foliage[ci],
                        Some(TintKind::Water) => tint_water[ci],
                        None => [1.0, 1.0, 1.0],
                    };
                    let wx = ox + x as i32;
                    let wz = oz + z as i32;
                    let l = neighbour_light(wx, y as i32, wz) as u32;
                    let sky6 = ((l * 63 + SKY_FULL as u32 / 2) / SKY_FULL as u32).min(63);
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

                // Water surface: per-corner heights (so neighbouring cells join
                // into one continuous sloped sheet) plus the top tile + rotation
                // derived from the flow direction.
                let mut water_h = [[1.0f32; 2]; 2];
                let mut water_top_tile = Tile::WaterStill;
                let mut water_top_angle = 0u32;
                // Full-height water (capped from above, or a falling column) fills
                // to the top and its sides render full height rather than sloping.
                let water_full =
                    is_water && water_fills_cell(ox + x as i32, y as i32, oz + z as i32);
                if is_water {
                    let wx = ox + x as i32;
                    let wz = oz + z as i32;
                    let yy = y as i32;
                    // 2x2 corner heights, indexed [cx][cz]: average the up-to-4
                    // water cells meeting at each corner.
                    for cx in 0..2i32 {
                        for cz in 0..2i32 {
                            let mut sum = 0.0;
                            let mut cnt = 0;
                            for ox2 in (cx - 1)..=cx {
                                for oz2 in (cz - 1)..=cz {
                                    if let Some(h) = fluid_at(wx + ox2, yy, wz + oz2) {
                                        sum += h;
                                        cnt += 1;
                                    }
                                }
                            }
                            water_h[cx as usize][cz as usize] =
                                if cnt == 0 { 1.0 } else { sum / cnt as f32 };
                        }
                    }
                    // Flow vector from the surface gradient: shared with entity
                    // physics so current push matches the texture heading.
                    let flow =
                        crate::world::water::surface_flow_dir(wx, yy, wz, &block_at, &fluid_at);
                    if flow.length_squared() > 0.0 {
                        water_top_tile = Tile::WaterFlow;
                        // Continuous flow heading: the shader rotates the flow tile
                        // by this angle so a cell streaming into a corner points
                        // diagonally, not snapped to a cardinal. atan2(x, z)
                        // keeps +Z=0/-X=-90/+X=+90/-Z=180 so the cardinals match the
                        // texture's built-in down-flow. Quantized to 8 bits.
                        let a = flow.x.atan2(flow.z);
                        let frac = a / std::f32::consts::TAU + 0.5;
                        water_top_angle = ((frac * 256.0) as i32).rem_euclid(256) as u32;
                    }
                }

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
                    let mut water_exposed_step = false;
                    if is_water && nb == Block::Water {
                        // Cull faces between two water cells, EXCEPT a submerged
                        // or falling cell's SIDE toward an open-surface neighbour:
                        // this cell is full to the top while the neighbour's
                        // surface is recessed, so the height difference is an
                        // exposed vertical step. Cull it and the floor shows
                        // through the gap; render it to bridge the two surfaces.
                        let nb_full = water_fills_cell(ox + nx, y as i32, oz + nz);
                        if is_side && water_full && !nb_full {
                            water_exposed_step = true;
                        } else {
                            continue;
                        }
                    }

                    // Material for this face: base tile + optional biome-tinted
                    // overlay + tint + texture rotation. Water tops use the still
                    // or flow tile (rotated toward the flow); water sides always
                    // use the downward-flowing tile. Grass block SIDES render as
                    // dirt + a grayscale grass overlay tinted by the same biome
                    // grass colour as the top. Everything else is the face's own
                    // tile, tinted only for grass-top/foliage/water.
                    let (base_tile, overlay_tile, tint) = if is_water {
                        let t = match face {
                            Face::PosY => water_top_tile,
                            Face::NegY => Tile::WaterStill,
                            _ => Tile::WaterFlow,
                        };
                        (t, None, tint_water[ci])
                    } else if block == Block::Grass && is_side {
                        (Tile::Dirt, Some(Tile::GrassSideOverlay), tint_grass[ci])
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
                        let tint = match tile_tint(t) {
                            Some(TintKind::Grass) => tint_grass[ci],
                            Some(TintKind::Foliage) => tint_foliage[ci],
                            Some(TintKind::Water) => tint_water[ci],
                            None => [1.0, 1.0, 1.0],
                        };
                        (t, None, tint)
                    };

                    // Build quad vertices in CCW order when viewed from outside.
                    // Positions are in world space (baked chunk origin) so each
                    // chunk renders at its actual world coordinates.
                    let base_y = y as f32;
                    let [mut p0, mut p1, mut p2, mut p3] = quad_for(face, base_x, base_y, base_z);

                    // Water vertices: TOP verts go to their corner's surface height
                    // so the top slopes and every side's top edge meets it exactly
                    // (a watertight, connected sheet). A full-height cell is full
                    // to the top, so its faces span the whole block.
                    //   - Exposed-step faces additionally pull their BOTTOM verts up
                    //     to the neighbour's surface (= the shared corner height), so
                    //     only the band above the neighbour is drawn (no water-behind-
                    //     water double-blend).
                    if is_water {
                        for p in [&mut p0, &mut p1, &mut p2, &mut p3] {
                            let cx = ((p[0] - base_x) as usize).min(1);
                            let cz = ((p[2] - base_z) as usize).min(1);
                            if p[1] > base_y + 0.5 {
                                p[1] = base_y + if water_full { 1.0 } else { water_h[cx][cz] };
                            } else if water_exposed_step {
                                p[1] = base_y + water_h[cx][cz];
                            }
                        }
                    }

                    // Per-vertex ambient occlusion AND smooth skylight share one
                    // neighbourhood: for each corner, the front voxel F = block+
                    // normal plus its two edge neighbours and the diagonal one.
                    // AO counts solid occluders (darker = more buried); skylight
                    // averages the light of the NON-opaque cells of that 2x2 (F is
                    // always non-opaque for an emitted face, so the average is
                    // well-defined). Both are packed per vertex and interpolated.
                    let (ux, uy, uz) = face.ao_u();
                    let (vx, vy, vz) = face.ao_v();
                    let fx = ox + x as i32 + dx;
                    let fy = y as i32 + dy;
                    let fz = oz + z as i32 + dz;
                    let f_l = neighbour_light(fx, fy, fz) as u32;
                    let mut ao = [3u32; 4];
                    let mut light6 = [63u32; 4];
                    for (i, &(su, sv)) in face.ao_signs().iter().enumerate() {
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
                        // AO counts opaque cubes AND leaves (canopy self-occlusion).
                        ao[i] = vertex_ao(b1.occludes_ao(), b2.occludes_ao(), bd.occludes_ao());

                        // Smooth skylight: mean of F + the surround cells that carry
                        // light (anything not fully opaque -- leaves included, since
                        // they still transmit light even though they occlude AO).
                        let mut sum = f_l;
                        let mut cnt = 1u32;
                        if !b1.is_opaque() {
                            sum += neighbour_light(e1x, e1y, e1z) as u32;
                            cnt += 1;
                        }
                        if !b2.is_opaque() {
                            sum += neighbour_light(e2x, e2y, e2z) as u32;
                            cnt += 1;
                        }
                        if !bd.is_opaque() {
                            sum += neighbour_light(dxx, dyy, dzz) as u32;
                            cnt += 1;
                        }
                        // avg in [0,SKY_FULL] -> 6-bit level in [0,63], integer
                        // round-half-up (no f32, to keep meshes byte-identical).
                        let denom = cnt * SKY_FULL as u32;
                        light6[i] = ((sum * 63 + denom / 2) / denom).min(63);
                    }

                    // Pack base tile + shade + optional overlay once per face; the
                    // corner (0..3), AO level (0..3) and skylight (0..63) are
                    // per-vertex. Bit layout:
                    //   0..8 base tile | 8..10 corner | 10..12 shade
                    //   12..20 overlay tile | 20 has-overlay | 21..23 AO
                    //   23..29 skylight
                    // The shader selects uvs from the CPU-baked tile_uv() table by
                    // (tile, corner): 0->(u0,v1) 1->(u1,v1) 2->(u1,v0) 3->(u0,v0).
                    // Water has no grass overlay, so a flowing TOP face reuses its
                    // per-face overlay-tile bits to carry the quantized flow heading
                    // (the `has-overlay` flag stays 0, so the fragment shader never
                    // composites an overlay). Side faces derive their texture V from
                    // the vertex height in the shader, so they need no data here.
                    let water_ov: u32 = if is_water && matches!(face, Face::PosY) {
                        water_top_angle
                    } else {
                        0
                    };
                    let (ov_tile, ov_flag) = match overlay_tile {
                        Some(o) => (o as u32, 1u32),
                        None => (water_ov, 0u32),
                    };
                    let face_bits = (base_tile as u32)
                        | (face.shade_idx() << 10)
                        | (ov_tile << 12)
                        | (ov_flag << 20);
                    let corners = [p0, p1, p2, p3];

                    let (vbuf, ibuf, sections) = if is_water {
                        (
                            &mut transparent,
                            &mut transparent_idx,
                            &mut transparent_sections,
                        )
                    } else {
                        (&mut opaque, &mut opaque_idx, &mut opaque_sections)
                    };

                    let start = vbuf.len() as u32;
                    for (corner, p) in corners.into_iter().enumerate() {
                        vbuf.push(Vertex {
                            pos: p,
                            tint,
                            packed: face_bits
                                | ((corner as u32) << 8)
                                | (ao[corner] << 21)
                                | (light6[corner] << 23),
                        });
                    }
                    // Flip the triangulation so the split runs along the darker
                    // diagonal -- keeps the AO gradient symmetric (no bright bleed).
                    let first_index = ibuf.len() as u32;
                    let tris: [u32; 6] = if should_flip(ao) {
                        [start, start + 1, start + 3, start + 1, start + 2, start + 3]
                    } else {
                        [start, start + 1, start + 2, start, start + 2, start + 3]
                    };
                    ibuf.extend_from_slice(&tris);
                    // The transparent pass is back-face culled, so the water SURFACE
                    // (top face) also needs the reverse winding to stay visible from
                    // underneath when submerged. Side/bottom faces stay single-sided.
                    if is_water && matches!(face, Face::PosY) {
                        ibuf.extend_from_slice(&[
                            tris[0], tris[2], tris[1], tris[3], tris[5], tris[4],
                        ]);
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
    // packed: 0..8 tile | 8..10 corner | 10..12 shade(0) | 12..20 overlay |
    //         20 has-overlay(0) | 21..23 AO | 23..29 skylight.
    let face_bits = tile as u32;
    for plane in cross_quads(bx, y, bz) {
        let start = opaque.len() as u32;
        for (corner, p) in plane.into_iter().enumerate() {
            opaque.push(Vertex {
                pos: p,
                tint,
                packed: face_bits | ((corner as u32) << 8) | (3u32 << 21) | (sky6 << 23),
            });
        }
        opaque_idx.extend_from_slice(&[start, start + 1, start + 2, start, start + 2, start + 3]);
        opaque_idx.extend_from_slice(&[start, start + 2, start + 1, start, start + 3, start + 2]);
    }
}
