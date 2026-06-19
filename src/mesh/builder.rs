use crate::atlas::Tile;
use crate::biome::Biome;
use crate::block::Block;
use crate::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SKY_FULL};

use super::face::{quad_for, should_flip, vertex_ao, Face, FACES};
use super::vertex::{ChunkMesh, Vertex};

#[derive(Copy, Clone)]
enum TintKind {
    Grass,
    Foliage,
    Water,
}

fn tile_tint(tile: Tile) -> Option<TintKind> {
    match tile {
        Tile::GrassTop => Some(TintKind::Grass),
        Tile::Water => Some(TintKind::Water),
        Tile::OakLeaves => Some(TintKind::Foliage),
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
    let mut opaque = vec![];
    let mut opaque_idx = vec![];
    let mut transparent = vec![];
    let mut transparent_idx = vec![];

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

                // Only water is alpha-blended; leaves render in the OPAQUE pass
                // (crisp/cutout, no see-through ghosting) per the "fully opaque" rule.
                let is_water = block == Block::Water;

                // Choose tile for each face.
                let [tile_top, tile_bot, tile_side] = block.tiles();
                let ci = z * CHUNK_SX + x;
                let base_x = x as f32 + ox as f32;
                let base_z = z as f32 + oz as f32;

                for face in FACES {
                    let (dx, dy, dz) = face.dir();
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    let nz = z as i32 + dz;

                    // Neighbour block to test cull.
                    let nb_id =
                        if nx < 0 || nx >= CHUNK_SX as i32 || nz < 0 || nz >= CHUNK_SZ as i32 {
                            // Out of horizontal chunk bounds -> ask neighbour fn.
                            let wx = ox + nx;
                            let wz = oz + nz;
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
                    if nb.is_opaque() {
                        continue;
                    }
                    if is_water && nb == Block::Water {
                        continue;
                    }

                    // Material for this face: base tile + optional biome-tinted
                    // overlay + tint. Grass block SIDES render as dirt + a
                    // grayscale grass overlay tinted by the same biome grass
                    // colour as the top, so side grass matches the top (the
                    // pre-greened grass_block_side never did). Everything else is
                    // the face's own tile, tinted only for grass-top/foliage/water.
                    let is_side = matches!(face, Face::PosX | Face::NegX | Face::PosZ | Face::NegZ);
                    let (base_tile, overlay_tile, tint) = if block == Block::Grass && is_side {
                        (Tile::Dirt, Some(Tile::GrassSideOverlay), tint_grass[ci])
                    } else {
                        let t = match face {
                            Face::PosY => tile_top,
                            Face::NegY => tile_bot,
                            _ => tile_side,
                        };
                        let tint = match tile_tint(t) {
                            Some(TintKind::Grass) => tint_grass[ci],
                            Some(TintKind::Foliage) => tint_foliage[ci],
                            Some(TintKind::Water) => tint_water[ci],
                            None => [1.0, 1.0, 1.0],
                        };
                        (t, None, tint)
                    };

                    // Water top face: lower the top by 0.1 to mimic MC water surface.
                    let y_adjust = if is_water && matches!(face, Face::PosY) {
                        -0.10
                    } else {
                        0.0
                    };

                    // Build quad vertices in CCW order when viewed from outside.
                    // Positions are in world space (baked chunk origin) so each
                    // chunk renders at its actual world coordinates.
                    let base_y = y as f32 + y_adjust;
                    let [p0, p1, p2, p3] = quad_for(face, base_x, base_y, base_z);

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
                    let (ov_tile, ov_flag) = match overlay_tile {
                        Some(o) => (o as u32, 1u32),
                        None => (0, 0),
                    };
                    let face_bits = (base_tile as u32)
                        | (face.shade_idx() << 10)
                        | (ov_tile << 12)
                        | (ov_flag << 20);
                    let corners = [p0, p1, p2, p3];

                    let (vbuf, ibuf) = if is_water {
                        (&mut transparent, &mut transparent_idx)
                    } else {
                        (&mut opaque, &mut opaque_idx)
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
                    if should_flip(ao) {
                        ibuf.extend_from_slice(&[
                            start,
                            start + 1,
                            start + 3,
                            start + 1,
                            start + 2,
                            start + 3,
                        ]);
                    } else {
                        ibuf.extend_from_slice(&[
                            start,
                            start + 1,
                            start + 2,
                            start,
                            start + 2,
                            start + 3,
                        ]);
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
        mesh_dirty: true,
    }
}
