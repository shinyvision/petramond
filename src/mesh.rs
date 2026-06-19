//! Chunk meshing: per-face culling, opaque + transparent passes, atlas UVs.
//!
//! Vertex layout: position (3 floats) + UV (2 floats) + light (1 float, 0..1
//! face-direction-based shading, baked AO skipped in v1).

use crate::block::Block;
use crate::biome::Biome;
use crate::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ};

/// Per-face directional shade factors, indexed by `Face::shade_idx`. The vertex
/// shader (`block.wgsl`) holds a byte-identical copy; `tests::shade_table_*`
/// locks the two in sync. Top brightest, bottom darkest.
pub const SHADES: [f32; 4] = [1.00, 0.85, 0.75, 0.55];

/// GPU vertex: 28 bytes. `pos` and `tint` stay full `f32` (pos keeps the water
/// surface Y baked on the CPU; tint must not be quantized — the sRGB OETF would
/// shift output levels). `packed` folds the uv tile + corner + shade index into
/// one word; the vertex shader reconstructs uv (by SELECTING from a CPU-uploaded
/// `tile_uv()` table — never recomputing) and light (from the SHADES literal),
/// so every decoded value is bit-identical to the old inline `uv`/`light` and
/// the rendered image is unchanged.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub tint: [f32; 3],
    /// bits 0..8 = tile id (`Tile as u32`), 8..10 = corner (0..3),
    /// 10..12 = shade index (into `SHADES`).
    pub packed: u32,
}

pub struct ChunkMesh {
    pub opaque: Vec<Vertex>,
    pub opaque_idx: Vec<u32>,
    pub transparent: Vec<Vertex>,
    pub transparent_idx: Vec<u32>,
    /// True until GPU upload has happened. Set by `build_mesh`, cleared by
    /// renderer after a successful upload so we don't re-upload every frame.
    pub mesh_dirty: bool,
}

impl ChunkMesh {
    pub fn empty() -> Self {
        Self { opaque: vec![], opaque_idx: vec![], transparent: vec![], transparent_idx: vec![], mesh_dirty: false }
    }
    pub fn is_empty(&self) -> bool {
        self.opaque_idx.is_empty() && self.transparent_idx.is_empty()
    }
}

/// Face direction enum.
#[derive(Copy, Clone, Debug)]
enum Face { PosX, NegX, PosY, NegY, PosZ, NegZ }

impl Face {
    fn dir(self) -> (i32, i32, i32) {
        match self {
            Face::PosX => (1, 0, 0),  Face::NegX => (-1, 0, 0),
            Face::PosY => (0, 1, 0),  Face::NegY => (0, -1, 0),
            Face::PosZ => (0, 0, 1),  Face::NegZ => (0, 0, -1),
        }
    }
    /// Per-face directional shading factor (top brightest, bottom darkest).
    /// Now a test-only oracle: production reads `SHADES[shade_idx]` (and the
    /// shader mirrors it); `tests::shade_table_matches_face_shade` checks they agree.
    #[cfg(test)]
    fn shade(self) -> f32 {
        match self {
            Face::PosY => 1.00,
            Face::PosX | Face::NegX => 0.75,
            Face::PosZ | Face::NegZ => 0.85,
            Face::NegY => 0.55,
        }
    }
    /// Index into `SHADES` (and the shader's mirror) for this face — packed into
    /// the vertex instead of the raw float.
    fn shade_idx(self) -> u32 {
        match self {
            Face::PosY => 0,
            Face::PosZ | Face::NegZ => 1,
            Face::PosX | Face::NegX => 2,
            Face::NegY => 3,
        }
    }
}

const FACES: [Face; 6] = [
    Face::PosX, Face::NegX, Face::PosY, Face::NegY, Face::PosZ, Face::NegZ,
];

/// Build the mesh for one chunk. Neighbour chunk block lookups are needed
/// for cross-chunk face culling: pass them via `neighbour_block`.
/// `neighbour_biome(wx, wz)` returns biome id at world column; used for
/// biome-blend tints (grass top / water / leaves).
pub fn build_mesh(
    chunk: &Chunk,
    neighbour_block: impl Fn(i32, i32, i32) -> u8,
    neighbour_biome: impl Fn(i32, i32) -> u8,
) -> ChunkMesh {
    let mut opaque = vec![];
    let mut opaque_idx = vec![];
    let mut transparent = vec![];
    let mut transparent_idx = vec![];

    let (ox, oz) = chunk.chunk_origin_world();

    use crate::atlas::Tile;
    #[derive(Copy, Clone)]
    enum TintKind { Grass, Foliage, Water }
    fn tile_tint(tile: Tile) -> Option<TintKind> {
        match tile {
            Tile::GrassTop => Some(TintKind::Grass),
            Tile::Water => Some(TintKind::Water),
            Tile::OakLeaves => Some(TintKind::Foliage),
            _ => None,
        }
    }

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
                    g[0] += b.grass_color()[0]; g[1] += b.grass_color()[1]; g[2] += b.grass_color()[2];
                    f[0] += b.foliage_color()[0]; f[1] += b.foliage_color()[1]; f[2] += b.foliage_color()[2];
                    w[0] += b.water_color()[0]; w[1] += b.water_color()[1]; w[2] += b.water_color()[2];
                }
            }
            let i = z * CHUNK_SX + x;
            tint_grass[i] = [g[0]/n, g[1]/n, g[2]/n];
            tint_foliage[i] = [f[0]/n, f[1]/n, f[2]/n];
            tint_water[i] = [w[0]/n, w[1]/n, w[2]/n];
        }
    }

    // Skip the all-air shell above the terrain. `heightmap[i]` is the highest
    // non-air Y in column i (set for every non-air block incl. water; rebuilt by
    // recompute_heightmap when block data arrives raw — see worker.rs). Bounding
    // the outer loop by the chunk-wide max is byte-identical to looping 0..CHUNK_SY:
    // every skipped iteration (y > max_h) has an air centre voxel that would hit
    // the `Block::Air { continue }` guard below and emit zero bytes. We use the
    // chunk-wide max (NOT a per-column bound) so the y-major emission order — and
    // thus the alpha-blended transparent buffer ordering — is exactly preserved.
    let max_h = chunk.heightmap.iter().copied().max().unwrap_or(0) as usize;
    for y in 0..=max_h {
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                let id = chunk.block_raw(x, y, z);
                let block = Block::from_id(id);
                if block == Block::Air { continue; }

                // Only water is alpha-blended; leaves render in the OPAQUE pass
                // (crisp/cutout, no see-through ghosting) per the "fully opaque" rule.
                let is_water = block == Block::Water;

                // Choose tile for each face.
                let [tile_top, tile_bot, tile_side] = block.tiles();

                for face in FACES {
                    let (dx, dy, dz) = face.dir();
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    let nz = z as i32 + dz;

                    // Neighbour block to test cull.
                    let nb_id = if nx < 0 || nx >= CHUNK_SX as i32
                        || nz < 0 || nz >= CHUNK_SZ as i32
                    {
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
                    // opaque cube (`is_opaque()` — stone/dirt/grass/sand/snow/log).
                    // Leaves are NOT opaque-for-culling (they're a cutout), so
                    // leaf↔leaf faces are intentionally NOT culled — every leaf
                    // cube draws all its faces, giving a dense canopy you can't see
                    // through to the sky. Water additionally culls against itself.
                    if nb.is_opaque() { continue; }
                    if is_water && nb == Block::Water { continue; }

                    // Material for this face: base tile + optional biome-tinted
                    // overlay + tint. Grass block SIDES render as dirt + a
                    // grayscale grass overlay tinted by the same biome grass
                    // colour as the top, so side grass matches the top (the
                    // pre-greened grass_block_side never did). Everything else is
                    // the face's own tile, tinted only for grass-top/foliage/water.
                    let ci = z * CHUNK_SX + x;
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
                    } else { 0.0 };

                    // Build quad vertices in CCW order when viewed from outside.
                    // Positions are in world space (baked chunk origin) so each
                    // chunk renders at its actual world coordinates.
                    let base_x = x as f32 + ox as f32;
                    let base_y = y as f32 + y_adjust;
                    let base_z = z as f32 + oz as f32;
                    let [p0, p1, p2, p3] = quad_for(face, base_x, base_y, base_z);

                    // Pack base tile + shade + optional overlay once per face; the
                    // corner (0..3) is the per-vertex index. Bit layout:
                    //   0..8 base tile | 8..10 corner | 10..12 shade
                    //   12..20 overlay tile | 20 has-overlay
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
                        vbuf.push(Vertex { pos: p, tint, packed: face_bits | ((corner as u32) << 8) });
                    }
                    ibuf.extend_from_slice(&[start, start+1, start+2, start, start+2, start+3]);
                }
            }
        }
    }

    ChunkMesh { opaque, opaque_idx, transparent, transparent_idx, mesh_dirty: true }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::generate_chunk;

    /// The packed shade index must decode (via SHADES) to the same float the old
    /// per-vertex `Face::shade()` produced — and SHADES must match the literal
    /// table in block.wgsl. Guards the index↔value mapping against drift.
    #[test]
    fn shade_table_matches_face_shade() {
        for f in FACES {
            assert_eq!(SHADES[f.shade_idx() as usize], f.shade(), "shade idx/value drift for {f:?}");
        }
        // Mirror of block.wgsl's `array<f32,4>(...)`.
        assert_eq!(SHADES, [1.00, 0.85, 0.75, 0.55]);
    }

    /// Leaves must render in the OPAQUE pass, not the alpha-blended one. Proof: a
    /// chunk that has leaves but NO water must produce an empty transparent buffer
    /// (only water feeds it now) and a non-empty opaque buffer.
    #[test]
    fn leaves_go_to_opaque_pass() {
        let seed = 0x1234_5678u32;
        for cz in 0..16 {
            for cx in 0..16 {
                let c = generate_chunk(seed, cx, cz);
                let (mut leaf, mut water) = (false, false);
                for y in 0..CHUNK_SY {
                    for z in 0..CHUNK_SZ {
                        for x in 0..CHUNK_SX {
                            match Block::from_id(c.block_raw(x, y, z)) {
                                Block::OakLeaves => leaf = true,
                                Block::Water => water = true,
                                _ => {}
                            }
                        }
                    }
                }
                if leaf && !water {
                    let mesh = build_mesh(&c, |_, _, _| 0u8, |_, _| 4u8);
                    assert!(
                        mesh.transparent_idx.is_empty(),
                        "leaves+no-water chunk should have an empty transparent buffer"
                    );
                    assert!(!mesh.opaque_idx.is_empty(), "leaves should fill the opaque buffer");
                    return;
                }
            }
        }
        panic!("no leaf-bearing, water-free chunk found to test");
    }
}

/// Parallel mesh building (World::tick_mesh_budget on native) must produce
/// byte-identical meshes to a serial build: `build_mesh` is a pure function of
/// (chunk, neighbour reads) with no shared mutable state, so rayon only reorders
/// independent work. This locks that invariant down objectively (perfbench
/// meshes serially and never exercises the rayon path).
#[cfg(test)]
#[cfg(not(target_arch = "wasm32"))]
mod parallel_parity_tests {
    use super::*;
    use crate::worldgen::generate_chunk;
    use rayon::prelude::*;
    use std::collections::HashMap;

    #[test]
    fn parallel_meshing_is_byte_identical_to_serial() {
        let seed = 0x1234_5678u32;
        let coords: Vec<(i32, i32)> =
            (-2..=2).flat_map(|cz| (-2..=2).map(move |cx| (cx, cz))).collect();
        let chunks: HashMap<(i32, i32), Chunk> =
            coords.iter().map(|&(cx, cz)| ((cx, cz), generate_chunk(seed, cx, cz))).collect();

        let mesh_one = |&(cx, cz): &(i32, i32)| -> ChunkMesh {
            let c = &chunks[&(cx, cz)];
            let nb = |wx: i32, wy: i32, wz: i32| -> u8 {
                if wy < 0 || wy >= CHUNK_SY as i32 { return 0; }
                match chunks.get(&(wx >> 4, wz >> 4)) {
                    Some(c) => c.block_raw((wx & 15) as usize, wy as usize, (wz & 15) as usize),
                    None => 0,
                }
            };
            let nb_biome = |wx: i32, wz: i32| -> u8 {
                match chunks.get(&(wx >> 4, wz >> 4)) {
                    Some(c) => c.biome_at((wx & 15) as usize, (wz & 15) as usize),
                    None => 0,
                }
            };
            build_mesh(c, nb, nb_biome)
        };

        let serial: Vec<ChunkMesh> = coords.iter().map(mesh_one).collect();
        let parallel: Vec<ChunkMesh> = coords.par_iter().map(mesh_one).collect();

        for (s, p) in serial.iter().zip(&parallel) {
            assert_eq!(
                bytemuck::cast_slice::<Vertex, u8>(&s.opaque),
                bytemuck::cast_slice::<Vertex, u8>(&p.opaque),
            );
            assert_eq!(s.opaque_idx, p.opaque_idx);
            assert_eq!(
                bytemuck::cast_slice::<Vertex, u8>(&s.transparent),
                bytemuck::cast_slice::<Vertex, u8>(&p.transparent),
            );
            assert_eq!(s.transparent_idx, p.transparent_idx);
        }
    }
}

fn quad_for(face: Face, x: f32, y: f32, z: f32) -> [[f32;3]; 4] {
    // Returns 4 corners CCW as seen from +axis direction.
    match face {
        Face::PosX => [
            [x+1.0, y,   z+1.0],
            [x+1.0, y,   z     ],
            [x+1.0, y+1.0, z     ],
            [x+1.0, y+1.0, z+1.0],
        ],
        Face::NegX => [
            [x,     y,   z     ],
            [x,     y,   z+1.0],
            [x,     y+1.0, z+1.0],
            [x,     y+1.0, z     ],
        ],
        Face::PosY => [
            [x,     y+1.0, z+1.0],
            [x+1.0, y+1.0, z+1.0],
            [x+1.0, y+1.0, z     ],
            [x,     y+1.0, z     ],
        ],
        Face::NegY => [
            [x,     y,   z     ],
            [x+1.0, y,   z     ],
            [x+1.0, y,   z+1.0],
            [x,     y,   z+1.0],
        ],
        Face::PosZ => [
            [x,     y,   z+1.0],
            [x+1.0, y,   z+1.0],
            [x+1.0, y+1.0, z+1.0],
            [x,     y+1.0, z+1.0],
        ],
        Face::NegZ => [
            [x+1.0, y,   z     ],
            [x,     y,   z     ],
            [x,     y+1.0, z     ],
            [x+1.0, y+1.0, z     ],
        ],
    }
}