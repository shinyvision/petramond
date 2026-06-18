//! Chunk meshing: per-face culling, opaque + transparent passes, atlas UVs.
//!
//! Vertex layout: position (3 floats) + UV (2 floats) + light (1 float, 0..1
//! face-direction-based shading, baked AO skipped in v1).

use crate::atlas::tile_uv;
use crate::block::Block;
use crate::biome::Biome;
use crate::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ};

#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    pub light: f32,
    pub tint: [f32; 3],
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
    fn shade(self) -> f32 {
        match self {
            Face::PosY => 1.00,
            Face::PosX | Face::NegX => 0.75,
            Face::PosZ | Face::NegZ => 0.85,
            Face::NegY => 0.55,
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

    for y in 0..CHUNK_SY {
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                let id = chunk.block_raw(x, y, z);
                let block = Block::from_id(id);
                if block == Block::Air { continue; }

                let is_transparent = block.is_transparent();
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

                    // Cull rule: do not draw face if neighbour opaque.
                    // For transparent blocks: cull against same-type only
                    // (water-water cull, leaves-leaves cull to save tris).
                    if !is_transparent {
                        if nb.is_opaque() { continue; }
                    } else {
                        // Don't cull water by leaves (different type).
                        if nb == block { continue; }
                        if nb.is_opaque() { continue; }
                        // water against air: draw.
                    }

                    // Select tile by face direction.
                    let tile = match face {
                        Face::PosY => tile_top,
                        Face::NegY => tile_bot,
                        _ => tile_side,
                    };
                    let uv = tile_uv(tile);

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

                    let light = face.shade();

                    // Biome-blend tint: tile-tinted kinds only (grass top, water, leaves).
                    let tint = match tile_tint(tile) {
                        Some(TintKind::Grass) => tint_grass[z * CHUNK_SX + x],
                        Some(TintKind::Foliage) => tint_foliage[z * CHUNK_SX + x],
                        Some(TintKind::Water) => tint_water[z * CHUNK_SX + x],
                        None => [1.0, 1.0, 1.0],
                    };

                    // Per-vertex UV: map tile rect to quad corners.
                    // Order matches quad_for ordering: (u0,v0)(u1,v0)(u1,v1)(u0,v1)
                    // where (u0,v0) = top-left of tile in atlas (v flipped).
                    let [u0, v0, u1, v1] = uv;
                    let verts = [
                        (p0, [u0, v1]),
                        (p1, [u1, v1]),
                        (p2, [u1, v0]),
                        (p3, [u0, v0]),
                    ];

                    let (vbuf, ibuf) = if is_transparent {
                        (&mut transparent, &mut transparent_idx)
                    } else {
                        (&mut opaque, &mut opaque_idx)
                    };

                    let start = vbuf.len() as u32;
                    for (p, uv) in verts {
                        vbuf.push(Vertex { pos: p, uv, light, tint });
                    }
                    ibuf.extend_from_slice(&[start, start+1, start+2, start, start+2, start+3]);
                }
            }
        }
    }

    ChunkMesh { opaque, opaque_idx, transparent, transparent_idx, mesh_dirty: true }
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