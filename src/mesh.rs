//! Chunk meshing: per-face culling, opaque + transparent passes, atlas UVs.
//!
//! Lighting is `directional face shade x per-vertex ambient occlusion`: the
//! face-direction `SHADES` factor (top brightest, bottom darkest) is modulated
//! by a Minecraft-style "smooth lighting" AO term baked per vertex from the
//! solid neighbours around each corner. The shader interpolates the per-vertex
//! AO across the face, giving the soft contact shadows in nooks and against
//! adjacent blocks.

use std::cell::RefCell;

use crate::block::Block;
use crate::biome::Biome;
use crate::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SKY_FULL};

thread_local! {
    /// Reusable skylight scratch (the medium buffer + the Dijkstra bucket queues),
    /// kept per worker thread so the per-chunk flood-fill doesn't churn the
    /// allocator across thousands of streaming mesh builds. Cleared each use; the
    /// result buffer (`light2`) is allocated fresh since it outlives the solve.
    static SKY_SCRATCH: RefCell<(Vec<u8>, Vec<Vec<u32>>)> = const { RefCell::new((Vec::new(), Vec::new())) };
}

/// Per-face directional shade factors, indexed by `Face::shade_idx`. The vertex
/// shader (`block.wgsl`) holds a byte-identical copy; `tests::shade_table_*`
/// locks the two in sync. Top brightest, bottom darkest.
pub const SHADES: [f32; 4] = [1.00, 0.85, 0.75, 0.55];

/// GPU vertex: 28 bytes. `pos` and `tint` stay full `f32` (pos keeps the water
/// surface Y baked on the CPU; tint must not be quantized — the sRGB OETF would
/// shift output levels). `packed` folds the uv tile + corner + shade index + AO
/// level into one word; the vertex shader reconstructs uv (by SELECTING from a
/// CPU-uploaded `tile_uv()` table — never recomputing) and light (from the
/// `SHADES` literal times an AO lookup). The uv/shade decode is bit-identical to
/// the old inline values; `light` additionally folds in the per-vertex AO term.
#[repr(C)]
#[derive(Copy, Clone, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub tint: [f32; 3],
    /// bits 0..8 = tile id (`Tile as u32`), 8..10 = corner (0..3),
    /// 10..12 = shade index (into `SHADES`), 12..20 = overlay tile,
    /// 20 = has-overlay flag, 21..23 = AO level (0 dark .. 3 bright),
    /// 23..29 = skylight level (0 dark .. 63 full sky).
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

    /// First tangent axis (unit vector) used when sampling AO occluders — one of
    /// the two world axes perpendicular to the face normal.
    fn ao_u(self) -> (i32, i32, i32) {
        match self {
            Face::PosX | Face::NegX => (0, 1, 0), // Y
            Face::PosY | Face::NegY => (1, 0, 0), // X
            Face::PosZ | Face::NegZ => (1, 0, 0), // X
        }
    }
    /// Second tangent axis (unit vector) for AO occluder sampling.
    fn ao_v(self) -> (i32, i32, i32) {
        match self {
            Face::PosX | Face::NegX => (0, 0, 1), // Z
            Face::PosY | Face::NegY => (0, 0, 1), // Z
            Face::PosZ | Face::NegZ => (0, 1, 0), // Y
        }
    }
    /// Per-corner tangent signs `(su, sv)` for the quad corners `p0..p3` in the
    /// same CCW order `quad_for` emits. `su`/`sv` pick which side along `ao_u`/
    /// `ao_v` (relative to the front voxel `block + normal`) each corner's three
    /// AO occluders sit on. Derived from `quad_for` and independently verified
    /// per face; keep in lockstep with `quad_for` if corner order ever changes.
    fn ao_signs(self) -> [(i32, i32); 4] {
        match self {
            Face::PosX => [(-1, 1), (-1, -1), (1, -1), (1, 1)],
            Face::NegX => [(-1, -1), (-1, 1), (1, 1), (1, -1)],
            Face::PosY => [(-1, 1), (1, 1), (1, -1), (-1, -1)],
            Face::NegY => [(-1, -1), (1, -1), (1, 1), (-1, 1)],
            Face::PosZ => [(-1, -1), (1, -1), (1, 1), (-1, 1)],
            Face::NegZ => [(1, -1), (-1, -1), (-1, 1), (1, 1)],
        }
    }
}

/// Minecraft per-vertex AO occlusion level: 0 = darkest (corner buried in a
/// crevice), 3 = no occlusion. `side1`/`side2` are the two edge-adjacent
/// neighbours of the corner in the voxel plane just outside the face; `corner`
/// is the diagonal one. Two solid edges bury the corner regardless of the
/// diagonal, so that case is forced to 0 (the well-known special case).
#[inline]
fn vertex_ao(side1: bool, side2: bool, corner: bool) -> u32 {
    if side1 && side2 { 0 } else { 3 - (side1 as u32 + side2 as u32 + corner as u32) }
}

/// Pick the quad's triangulation diagonal. Default splits along corners 0-2;
/// flip to the 1-3 diagonal when 0-2 is the brighter pair, so the seam runs
/// along the darker diagonal and the interpolated AO gradient stays symmetric
/// (the standard voxel-AO anisotropy fix). Strict `>` leaves ties on the default.
#[inline]
fn should_flip(ao: [u32; 4]) -> bool {
    ao[0] + ao[2] > ao[1] + ao[3]
}

// --- Skylight (Minecraft-style flood-fill, cached per chunk) -------------------
// Each chunk's skylight is computed from ITS OWN blocks (no neighbour reads),
// stored on the Chunk, and recomputed only when that chunk changes (see
// world.rs). Light is on an x2 integer scale (`SKY_FULL` = 30 = level 15): open
// sky = 15. Two terms:
//
//  * Vertical sky descent (pass 1, per column) is VOLUMETRIC: a running
//    attenuation `rate` ratchets up the moment skylight enters cover and then
//    keeps draining `rate` per block of DESCENT, even through the air beneath —
//    so it gets darker the deeper you go under water/leaves (and so digging a
//    shaft straight down under cover keeps darkening). Open air above any cover
//    has rate 0 (sky shafts stay 15 to the first cover/opaque block); a canopy
//    sets rate 0.5/block, water sets 1/block (water dominates leaves). The first
//    opaque block ends the column (no sky below it).
//  * Horizontal/secondary bleed (pass 2, bucketed Dijkstra) lights enclosed
//    spaces light can bend into — caves, tunnels, overhang mouths — at 1 level
//    per air/water step, half per leaf step. It only FILLS cells the sky descent
//    never reached (below the first opaque); it never re-brightens a sky-lit
//    cell, so it cannot flatten the volumetric depth gradient from the side.
//
// Being self-contained, horizontal light does NOT bleed across chunk borders —
// the dominant vertical sky term stays seamless (per-column); only secondary
// bleed into enclosed spaces can step at a border, and per-vertex border faces
// blend both sides to soften it.

/// How far below the lowest surface to keep solving, so overhang/cave-mouth spill
/// light is captured. Anything deeper just floors to the dark minimum.
const LIGHT_MARGIN_DOWN: i32 = 24;

// Medium codes for the flood buffer.
const M_AIR: u8 = 0;    // descent keeps the running rate; horizontal step costs 1 level
const M_LEAF: u8 = 1;   // canopy: sets descent rate >= 0.5/block; horizontal step costs 0.5
const M_WATER: u8 = 2;  // water: sets descent rate to 1/block; horizontal step costs 1
const M_OPAQUE: u8 = 3; // full cube: blocks light, breaks sky shafts

/// Compute the skylight band for `chunk` from its own blocks. Returns the flat
/// band buffer (x2 light, indexed like blocks with Y offset by `ylo`) plus the
/// band `[ylo, yhi]`. Pure integer flood-fill, order-independent -> deterministic.
/// Reuses per-thread scratch. Call when the chunk's blocks change; the result is
/// stored via `Chunk::set_skylight` and reused across mesh rebuilds.
pub fn compute_chunk_skylight(chunk: &Chunk) -> (Box<[u8]>, i32, i32) {
    const SX: i32 = CHUNK_SX as i32;
    const SZ: i32 = CHUNK_SZ as i32;

    // Vertical band from this chunk's own heightmap.
    let mut hmax = 0i32;
    let mut hmin = CHUNK_SY as i32 - 1;
    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            let h = chunk.surface_y(x, z);
            if h > hmax { hmax = h; }
            if h < hmin { hmin = h; }
        }
    }
    let yhi = (hmax + 1).min(CHUNK_SY as i32 - 1);
    let ylo = (hmin - LIGHT_MARGIN_DOWN).max(0);
    let bh = (yhi - ylo + 1).max(1);
    let vol = (SX * SZ * bh) as usize;

    // Temporary buffers from per-thread scratch (medium is fully overwritten by
    // the fill pass; buckets are cleared). The result band is allocated fresh.
    let (mut medium, mut buckets) = SKY_SCRATCH.with(|s| {
        let mut s = s.borrow_mut();
        (std::mem::take(&mut s.0), std::mem::take(&mut s.1))
    });
    medium.clear();
    medium.resize(vol, M_AIR);
    buckets.resize_with(SKY_FULL as usize + 1, Vec::new);
    for b in buckets.iter_mut() { b.clear(); }
    let mut light2 = vec![0u8; vol];
    // Marks cells reached by the vertical sky descent (pass 1). Their value is the
    // authoritative volumetric depth term; pass 2's horizontal bleed must not
    // re-brighten them, or an adjacent bright column (e.g. a dug shaft) would
    // flatten the depth gradient back to surface level.
    let mut sky = vec![false; vol];

    let idx = |x: i32, ay: i32, z: i32| -> usize { ((ay * SZ + z) * SX + x) as usize };

    // Pass 1: fill medium + seed the VOLUMETRIC sky descent. Descend each column
    // from the band top carrying a running `rate` (attenuation per block of
    // descent, x2 scale): rate 0 in open air above any cover, then it ratchets up
    // to 1 (0.5/block) under a canopy and 2 (1/block) under water and KEEPS
    // draining through the air below — so it gets darker the deeper you go under
    // cover. The first opaque block ends the column (no sky below it; `medium`
    // keeps filling so pass 2 can re-enter caves from the side).
    for z in 0..SZ {
        for x in 0..SX {
            let mut blocked = false;
            let mut cur = SKY_FULL;
            let mut rate = 0u8; // per-block descent attenuation, x2 (0 open / 1 leaf / 2 water)
            let mut wy = yhi;
            while wy >= ylo {
                let b = Block::from_id(chunk.block_raw(x as usize, wy as usize, z as usize));
                let m = if b.is_opaque() {
                    M_OPAQUE
                } else if b == Block::Water {
                    M_WATER
                } else if b == Block::OakLeaves {
                    M_LEAF
                } else {
                    M_AIR
                };
                let i = idx(x, wy - ylo, z);
                medium[i] = m;
                if !blocked {
                    if m == M_OPAQUE {
                        blocked = true;
                    } else {
                        // Cover ratchets the rate up (water dominates leaves);
                        // open air keeps whatever rate is already in effect.
                        rate = rate.max(match m { M_WATER => 2, M_LEAF => 1, _ => 0 });
                        cur = cur.saturating_sub(rate);
                        light2[i] = cur;
                        sky[i] = true;
                        buckets[cur as usize].push(i as u32);
                    }
                }
                wy -= 1;
            }
        }
    }

    // Pass 2: bucketed Dijkstra (bright -> dark) within the 16x16xbh box. Air/water
    // neighbour costs 2, leaf 1; opaque impassable. Sky-lit cells are frozen (their
    // pass-1 depth value is authoritative) — they still SOURCE light into enclosed
    // neighbours but are never raised. Staleness check skips voxels already improved
    // past their bucket. Final values are order-independent.
    let mut level = SKY_FULL as i32;
    while level >= 1 {
        while let Some(i) = buckets[level as usize].pop() {
            let iu = i as usize;
            if light2[iu] != level as u8 { continue; }
            let x = (i % SX as u32) as i32;
            let rem = i / SX as u32;
            let z = (rem % SZ as u32) as i32;
            let ay = (rem / SZ as u32) as i32;
            for (dx, dy, dz) in [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)] {
                let nx = x + dx;
                let ny = ay + dy;
                let nz = z + dz;
                if nx < 0 || nx >= SX || ny < 0 || ny >= bh || nz < 0 || nz >= SZ { continue; }
                let ni = idx(nx, ny, nz);
                if sky[ni] { continue; }
                let m = medium[ni];
                if m == M_OPAQUE { continue; }
                let step = if m == M_LEAF { 1 } else { 2 };
                if level > step {
                    let nl = (level - step) as u8;
                    if nl > light2[ni] {
                        light2[ni] = nl;
                        buckets[nl as usize].push(ni as u32);
                    }
                }
            }
        }
        level -= 1;
    }

    // Hand the temporary buffers back for the next build on this thread.
    SKY_SCRATCH.with(|s| {
        let mut s = s.borrow_mut();
        s.0 = medium;
        s.1 = buckets;
    });

    (light2.into_boxed_slice(), ylo, yhi)
}

const FACES: [Face; 6] = [
    Face::PosX, Face::NegX, Face::PosY, Face::NegY, Face::PosZ, Face::NegZ,
];

/// Build the mesh for one chunk. Neighbour chunk block lookups are needed for
/// cross-chunk face culling: pass them via `neighbour_block`.
/// `neighbour_biome(wx, wz)` returns biome id at world column; used for
/// biome-blend tints (grass top / water / leaves). `neighbour_light(wx, wy, wz)`
/// returns the cached skylight (x2 scale) at a world voxel — routed to the owning
/// chunk's stored band — so meshing just SAMPLES light, never recomputes it.
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
        if wy < 0 || wy >= CHUNK_SY as i32 { return Block::Air; }
        let lx = wx - ox;
        let lz = wz - oz;
        let id = if lx >= 0 && lx < CHUNK_SX as i32 && lz >= 0 && lz < CHUNK_SZ as i32 {
            chunk.block_raw(lx as usize, wy as usize, lz as usize)
        } else {
            neighbour_block(wx, wy, wz)
        };
        Block::from_id(id)
    };


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
                        let (dxx, dyy, dzz) = (fx + su * ux + sv * vx, fy + su * uy + sv * vy, fz + su * uz + sv * vz);
                        let b1 = block_at(e1x, e1y, e1z);
                        let b2 = block_at(e2x, e2y, e2z);
                        let bd = block_at(dxx, dyy, dzz);
                        // AO counts opaque cubes AND leaves (canopy self-occlusion).
                        ao[i] = vertex_ao(b1.occludes_ao(), b2.occludes_ao(), bd.occludes_ao());

                        // Smooth skylight: mean of F + the surround cells that carry
                        // light (anything not fully opaque — leaves included, since
                        // they still transmit light even though they occlude AO).
                        let mut sum = f_l;
                        let mut cnt = 1u32;
                        if !b1.is_opaque() { sum += neighbour_light(e1x, e1y, e1z) as u32; cnt += 1; }
                        if !b2.is_opaque() { sum += neighbour_light(e2x, e2y, e2z) as u32; cnt += 1; }
                        if !bd.is_opaque() { sum += neighbour_light(dxx, dyy, dzz) as u32; cnt += 1; }
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
                            pos: p, tint,
                            packed: face_bits
                                | ((corner as u32) << 8)
                                | (ao[corner] << 21)
                                | (light6[corner] << 23),
                        });
                    }
                    // Flip the triangulation so the split runs along the darker
                    // diagonal — keeps the AO gradient symmetric (no bright bleed).
                    if should_flip(ao) {
                        ibuf.extend_from_slice(&[start, start+1, start+3, start+1, start+2, start+3]);
                    } else {
                        ibuf.extend_from_slice(&[start, start+1, start+2, start, start+2, start+3]);
                    }
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
                let mut c = generate_chunk(seed, cx, cz);
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
                    let mesh = mesh_solo(&mut c);
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

    /// Sampler over a computed skylight band, for the skylight unit tests.
    struct TestSky { band: Box<[u8]>, ylo: i32, yhi: i32 }
    impl TestSky {
        fn at(&self, x: i32, y: i32, z: i32) -> u8 {
            if y > self.yhi { return SKY_FULL; }
            if y < self.ylo { return 0; }
            let ay = y - self.ylo;
            self.band[((ay * CHUNK_SZ as i32 + z) * CHUNK_SX as i32 + x) as usize]
        }
    }
    fn solo_skylight(c: &Chunk) -> TestSky {
        let (band, ylo, yhi) = compute_chunk_skylight(c);
        TestSky { band, ylo, yhi }
    }

    /// Mesh a standalone chunk: bake its self-contained skylight, then build the
    /// mesh sampling that cached light (out-of-chunk reads as open sky).
    fn mesh_solo(c: &mut Chunk) -> ChunkMesh {
        let (band, ylo, yhi) = compute_chunk_skylight(c);
        c.set_skylight(band, ylo, yhi);
        build_mesh(&*c, |_, _, _| 0u8, |_, _| 4u8, |wx, wy, wz| {
            if wx < 0 || wx >= CHUNK_SX as i32 || wz < 0 || wz >= CHUNK_SZ as i32
                || wy < 0 || wy >= CHUNK_SY as i32
            {
                SKY_FULL
            } else {
                c.skylight_at(wx as usize, wy, wz as usize)
            }
        })
    }

    /// Open columns are full sky (15 = 30 on the x2 scale), and nothing exceeds it.
    #[test]
    fn skylight_open_column_is_full() {
        let mut c = Chunk::new(0, 0);
        for z in 0..CHUNK_SZ { for x in 0..CHUNK_SX { c.set_block(x, 0, z, Block::Stone); } }
        let sky = solo_skylight(&c);
        // Air directly above the floor, open to the sky -> full light.
        assert_eq!(sky.at(8, 1, 8), SKY_FULL);
        // Nothing ever exceeds full sky.
        assert!(sky.band.iter().all(|&v| v <= SKY_FULL));
    }

    /// A sealed horizontal tunnel off an open vertical shaft: light falls off by
    /// `-1/block` (= -2 on the x2 scale) into the tunnel — the gradient the
    /// feature is built on. Fully enclosed in stone so the open apron of a
    /// standalone chunk can't leak light in and flatten it.
    #[test]
    fn skylight_tunnel_falls_off_by_one_per_block() {
        let mut c = Chunk::new(0, 0);
        // Solid stone slab y=0..=6 across the whole chunk.
        for z in 0..CHUNK_SZ { for x in 0..CHUNK_SX { for y in 0..=6 { c.set_block(x, y, z, Block::Stone); } } }
        // Vertical shaft open to the sky at (8,*,8).
        for y in 1..=6 { c.set_block(8, y, 8, Block::Air); }
        // Horizontal tunnel at y=3 running +x off the shaft.
        for x in 9..=13 { c.set_block(x, 3, 8, Block::Air); }
        let sky = solo_skylight(&c);
        assert_eq!(sky.at(8, 3, 8), SKY_FULL, "open shaft is full sky");
        // Each air block into the tunnel costs 2 on the x2 scale (= 1 real).
        assert_eq!(sky.at(9, 3, 8), SKY_FULL - 2);
        assert_eq!(sky.at(10, 3, 8), SKY_FULL - 4);
        assert_eq!(sky.at(11, 3, 8), SKY_FULL - 6);
        // Monotonically darker deeper in.
        assert!(sky.at(13, 3, 8) < sky.at(9, 3, 8));
    }

    /// Build an opaque-walled vertical shaft of `fill` from y=1..=8 over a floor,
    /// so the only light path is straight down through `fill`.
    fn walled_shaft(fill: Block) -> Chunk {
        let mut c = Chunk::new(0, 0);
        for z in 0..CHUNK_SZ { for x in 0..CHUNK_SX { c.set_block(x, 0, z, Block::Stone); } }
        for y in 1..=8 {
            c.set_block(8, y, 8, fill);
            c.set_block(7, y, 8, Block::Stone);
            c.set_block(9, y, 8, Block::Stone);
            c.set_block(8, y, 7, Block::Stone);
            c.set_block(8, y, 9, Block::Stone);
        }
        c
    }

    /// Water attenuates a FULL light level per layer (2 on the x2 scale = 1 real),
    /// the same rate as air — so light drops off quickly underwater.
    #[test]
    fn skylight_water_attenuates_one_level_per_layer() {
        let sky = solo_skylight(&walled_shaft(Block::Water));
        assert_eq!(sky.at(8, 9, 8), SKY_FULL, "air above the water is full sky");
        assert_eq!(sky.at(8, 8, 8), SKY_FULL - 2);
        assert_eq!(sky.at(8, 7, 8), SKY_FULL - 4);
        assert_eq!(sky.at(8, 6, 8), SKY_FULL - 6);
    }

    /// Leaves still attenuate at HALF rate (1 on the x2 scale = 0.5 real), so
    /// light reaches deeper into a canopy than into water.
    #[test]
    fn skylight_leaves_attenuate_half() {
        let sky = solo_skylight(&walled_shaft(Block::OakLeaves));
        assert_eq!(sky.at(8, 9, 8), SKY_FULL, "air above the leaves is full sky");
        assert_eq!(sky.at(8, 8, 8), SKY_FULL - 1);
        assert_eq!(sky.at(8, 7, 8), SKY_FULL - 2);
        assert_eq!(sky.at(8, 6, 8), SKY_FULL - 3);
    }

    /// Leaves occlude AO onto/within themselves: a solid leaf cluster floating in
    /// air must produce darkened (ao < 3) leaf faces — interior faces are buried
    /// by surrounding leaves. (Before, leaves never occluded, so AO stayed 3.)
    #[test]
    fn leaves_self_occlude() {
        assert!(Block::OakLeaves.occludes_ao());
        assert!(!Block::Water.occludes_ao());
        assert!(!Block::Air.occludes_ao());

        let mut c = Chunk::new(0, 0);
        for y in 5..=7 {
            for z in 7..=9 {
                for x in 7..=9 {
                    c.set_block(x, y, z, Block::OakLeaves);
                }
            }
        }
        let mesh = mesh_solo(&mut c);
        assert!(!mesh.opaque.is_empty(), "leaf cluster should mesh (cutout opaque pass)");
        let min_ao = mesh.opaque.iter().map(|v| (v.packed >> 21) & 0x3).min().unwrap();
        assert!(min_ao < 3, "leaves in a cluster must self-occlude (some ao < 3)");
    }

    /// The AO occlusion table: brightest with no occluders, one step per single
    /// occluder, and the buried-corner special case (both edges solid -> 0).
    #[test]
    fn vertex_ao_levels() {
        assert_eq!(vertex_ao(false, false, false), 3); // open
        assert_eq!(vertex_ao(true, false, false), 2);  // one edge
        assert_eq!(vertex_ao(false, false, true), 2);  // diagonal only
        assert_eq!(vertex_ao(true, false, true), 1);   // edge + diagonal
        assert_eq!(vertex_ao(true, true, false), 0);   // both edges -> buried
        assert_eq!(vertex_ao(true, true, true), 0);    // both edges, diagonal irrelevant
    }

    /// Flip exactly when the 0-2 diagonal is the brighter pair; ties keep default.
    #[test]
    fn flip_runs_along_darker_diagonal() {
        assert!(should_flip([3, 0, 3, 0]));   // 0-2 bright (6) vs 1-3 dark (0) -> flip
        assert!(!should_flip([0, 3, 0, 3]));  // 1-3 brighter -> keep default
        assert!(!should_flip([3, 3, 3, 3]));  // symmetric -> no flip
        assert!(!should_flip([2, 1, 1, 2]));  // equal sums (3 == 3) -> no flip
    }

    /// Stone floor (y=0..=4) over the whole chunk, so test columns are not open
    /// below — keeps the volumetric descent the only thing under study.
    fn floored_chunk() -> Chunk {
        let mut c = Chunk::new(0, 0);
        for z in 0..CHUNK_SZ { for x in 0..CHUNK_SX { for y in 0..=4 { c.set_block(x, y, z, Block::Stone); } } }
        c
    }

    /// Volumetric depth darkening: the air BELOW a leaf canopy keeps losing 0.5 a
    /// level (1 on the x2 scale) per block of descent, not just at the leaf — so it
    /// gets darker the deeper you go under cover (and digging down stays dark, see
    /// `skylight_digging_down_under_cover_keeps_darkening`).
    #[test]
    fn skylight_air_below_canopy_darkens_with_depth() {
        let mut c = floored_chunk();
        // A leaf roof at y=10 over the whole chunk; open air pocket y=5..=9 below.
        for z in 0..CHUNK_SZ { for x in 0..CHUNK_SX { c.set_block(x, 10, z, Block::OakLeaves); } }
        let sky = solo_skylight(&c);
        assert_eq!(sky.at(8, 11, 8), SKY_FULL, "open air above the canopy is full sky");
        assert_eq!(sky.at(8, 10, 8), SKY_FULL - 1, "the leaf itself drops half a level");
        // Each AIR block below the leaf keeps draining the under-canopy rate (1/block).
        assert_eq!(sky.at(8, 9, 8), SKY_FULL - 2);
        assert_eq!(sky.at(8, 8, 8), SKY_FULL - 3);
        assert_eq!(sky.at(8, 7, 8), SKY_FULL - 4);
        assert_eq!(sky.at(8, 6, 8), SKY_FULL - 5);
        assert_eq!(sky.at(8, 5, 8), SKY_FULL - 6);
    }

    /// Water drains a full level per block both THROUGH the water and on into the
    /// air pocket beneath it — the deeper under water, the darker.
    #[test]
    fn skylight_under_water_darkens_with_depth() {
        let mut c = floored_chunk();
        // Water body y=6..=10 over the whole chunk; open air pocket at y=5.
        for z in 0..CHUNK_SZ { for x in 0..CHUNK_SX { for y in 6..=10 { c.set_block(x, y, z, Block::Water); } } }
        let sky = solo_skylight(&c);
        assert_eq!(sky.at(8, 11, 8), SKY_FULL, "open air above the water is full sky");
        assert_eq!(sky.at(8, 10, 8), SKY_FULL - 2); // first water -1 level
        assert_eq!(sky.at(8, 6, 8), SKY_FULL - 10); // 5 water blocks -> -5 levels
        assert_eq!(sky.at(8, 5, 8), SKY_FULL - 12); // air below water keeps -1/block
    }

    /// Digging straight down under cover keeps getting darker: a shaft carved all
    /// the way through the floor under a leaf roof darkens monotonically to the
    /// bottom (the reported "digging down doesn't drop below the surface" bug).
    #[test]
    fn skylight_digging_down_under_cover_keeps_darkening() {
        let mut c = floored_chunk();
        for z in 0..CHUNK_SZ { for x in 0..CHUNK_SX { c.set_block(x, 10, z, Block::OakLeaves); } }
        for y in 0..=4 { c.set_block(8, y, 8, Block::Air); } // dig the floor out at (8,*,8)
        let sky = solo_skylight(&c);
        // Strictly darker each block down, from just under the leaf to the bottom.
        for y in 0..10 {
            assert!(
                sky.at(8, y, 8) < sky.at(8, y + 1, 8),
                "expected light at y={y} < y={}; got {} !< {}",
                y + 1, sky.at(8, y, 8), sky.at(8, y + 1, 8),
            );
        }
        assert_eq!(sky.at(8, 0, 8), SKY_FULL - 11, "bottom of the dug shaft is much darker");
    }

    /// Regression for the reported bug: an open dug shaft beside a water body must
    /// NOT flatten the water's depth gradient. Before the fix, horizontal bleed
    /// from the always-bright shaft re-lit the adjacent water to a constant level;
    /// the sky descent now freezes sky-lit cells so the gradient survives.
    #[test]
    fn skylight_depth_gradient_survives_adjacent_open_shaft() {
        let mut c = floored_chunk();
        for z in 0..CHUNK_SZ { for x in 0..CHUNK_SX { for y in 6..=10 { c.set_block(x, y, z, Block::Water); } } }
        // Dig a 1-wide shaft straight through the water at (8,8): re-opens to sky.
        for y in 6..=10 { c.set_block(8, y, 8, Block::Air); }
        let sky = solo_skylight(&c);
        // The shaft itself genuinely has sky access -> full sky all the way down.
        assert_eq!(sky.at(8, 10, 8), SKY_FULL);
        assert_eq!(sky.at(8, 6, 8), SKY_FULL);
        // The water column right next to it still darkens with depth (not flat).
        let col: Vec<u8> = (6..=10).rev().map(|y| sky.at(9, y, 8)).collect();
        assert_eq!(col, vec![SKY_FULL - 2, SKY_FULL - 4, SKY_FULL - 6, SKY_FULL - 8, SKY_FULL - 10]);
    }

    /// AO must actually be computed and vary: real terrain has both fully-lit
    /// (ao=3) corners and occluded (ao<3) ones. Scans a small chunk grid so the
    /// assertion can't hinge on one unlucky flat chunk.
    #[test]
    fn ao_varies_across_generated_terrain() {
        let seed = 0x1234_5678u32;
        let (mut saw_open, mut saw_occluded) = (false, false);
        'outer: for cz in 0..3 {
            for cx in 0..3 {
                let mut c = generate_chunk(seed, cx, cz);
                let mesh = mesh_solo(&mut c);
                for v in &mesh.opaque {
                    match (v.packed >> 21) & 0x3 {
                        3 => saw_open = true,
                        _ => saw_occluded = true,
                    }
                    if saw_open && saw_occluded { break 'outer; }
                }
            }
        }
        assert!(saw_open, "expected some fully-lit (ao=3) vertices");
        assert!(saw_occluded, "expected some occluded (ao<3) vertices in real terrain");
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

    /// The skylight bake runs under rayon (`World::poll`), so it must be
    /// deterministic: same blocks -> byte-identical band, regardless of thread or
    /// repetition (guards the per-thread `SKY_SCRATCH` being fully reset each call
    /// and the flood being order-independent).
    #[test]
    fn skylight_bake_is_deterministic_serial_vs_parallel() {
        let seed = 0x1234_5678u32;
        let coords: Vec<(i32, i32)> =
            (-2..=2).flat_map(|cz| (-2..=2).map(move |cx| (cx, cz))).collect();
        let chunks: Vec<Chunk> =
            coords.iter().map(|&(cx, cz)| generate_chunk(seed, cx, cz)).collect();

        let serial: Vec<(Box<[u8]>, i32, i32)> =
            chunks.iter().map(compute_chunk_skylight).collect();

        // Same chunk baked twice back-to-back on one thread -> identical (scratch reset).
        for (c, s) in chunks.iter().zip(&serial) {
            let again = compute_chunk_skylight(c);
            assert_eq!(&again.0[..], &s.0[..]);
            assert_eq!((again.1, again.2), (s.1, s.2));
        }

        // Parallel bake (mirrors World::poll) -> byte-identical to serial.
        let parallel: Vec<(Box<[u8]>, i32, i32)> =
            chunks.par_iter().map(compute_chunk_skylight).collect();
        for (p, s) in parallel.iter().zip(&serial) {
            assert_eq!(&p.0[..], &s.0[..], "parallel skylight bake differs from serial");
            assert_eq!((p.1, p.2), (s.1, s.2));
        }
    }

    #[test]
    fn parallel_meshing_is_byte_identical_to_serial() {
        let seed = 0x1234_5678u32;
        let coords: Vec<(i32, i32)> =
            (-2..=2).flat_map(|cz| (-2..=2).map(move |cx| (cx, cz))).collect();
        let chunks: HashMap<(i32, i32), Chunk> = coords
            .iter()
            .map(|&(cx, cz)| {
                let mut c = generate_chunk(seed, cx, cz);
                let (band, ylo, yhi) = compute_chunk_skylight(&c);
                c.set_skylight(band, ylo, yhi);
                ((cx, cz), c)
            })
            .collect();

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
            let nb_light = |wx: i32, wy: i32, wz: i32| -> u8 {
                if wy < 0 { return 0; }
                if wy >= CHUNK_SY as i32 { return SKY_FULL; }
                match chunks.get(&(wx >> 4, wz >> 4)) {
                    Some(c) => c.skylight_at((wx & 15) as usize, wy, (wz & 15) as usize),
                    None => SKY_FULL,
                }
            };
            build_mesh(c, nb, nb_biome, nb_light)
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