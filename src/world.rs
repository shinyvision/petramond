//! World: manages loaded chunks, requests async generation, serves
//! neighbour-block queries for meshing.
//!
//! Gen is off-thread: see `worker` module.
//! We keep two maps in sync: a `loaded` map for terrain-only chunks and
//! `meshes` for baked meshes. The worldgen worker emits finished chunks
//! that the main thread ingests each frame.

use std::collections::HashMap;

use crate::block::Block;
use crate::chunk::{Chunk, ChunkPos, CHUNK_SY, SKY_FULL};
use crate::mesh::{build_mesh, ChunkMesh};
use crate::worker::{GenRequest, WorkerPool};

pub const RENDER_DIST: i32 = 16;

pub struct World {
    pub seed: u32,
    pub chunks: HashMap<ChunkPos, Chunk>,
    pub meshes: HashMap<ChunkPos, ChunkMesh>,
    pub worker: WorkerPool,
    /// Chunks queued for gen (waiting on result).
    pub pending: HashMap<ChunkPos, ()>,
    pub render_dist: i32,
}

impl World {
    pub fn new(seed: u32, render_dist: i32) -> Self {
        Self {
            seed,
            chunks: HashMap::new(),
            meshes: HashMap::new(),
            worker: WorkerPool::new(seed),
            pending: HashMap::new(),
            render_dist,
        }
    }

    /// Update loaded chunk set around camera (in chunk coords).
    pub fn update_load(&mut self, cam_chunk_x: i32, cam_chunk_z: i32) {
        let r = self.render_dist;
        // Request all chunks within radius (Euclidean approx via squared).
        for dz in -r..=r {
            for dx in -r..=r {
                if dx*dx + dz*dz > r*r { continue; }
                let pos = ChunkPos::new(cam_chunk_x + dx, cam_chunk_z + dz);
                if self.chunks.contains_key(&pos) { continue; }
                if self.pending.contains_key(&pos) { continue; }
                self.worker.submit(GenRequest {
                    cx: pos.cx, cz: pos.cz, seed: self.seed,
                });
                self.pending.insert(pos, ());
            }
        }

        // Unload far chunks.
        let keep = r + 2;
        let cx = cam_chunk_x; let cz = cam_chunk_z;
        let to_drop: Vec<ChunkPos> = self.chunks.keys()
            .filter(|p| (p.cx - cx).abs() > keep || (p.cz - cz).abs() > keep)
            .cloned().collect();
        for p in to_drop {
            self.chunks.remove(&p);
            self.meshes.remove(&p);
            self.pending.remove(&p);
        }
    }

    /// Poll worker for finished chunks and ingest.
    /// Returns number of chunks ingested.
    pub fn poll(&mut self) -> usize {
        // Drain finished chunks from the worker.
        let mut fresh: Vec<(ChunkPos, Chunk)> = Vec::new();
        while let Some(res) = self.worker.try_recv() {
            let pos = ChunkPos::new(res.cx, res.cz);
            self.pending.remove(&pos);
            fresh.push((pos, res.chunk));
        }
        if fresh.is_empty() { return 0; }

        // Bake each fresh chunk's skylight ONCE, from its own blocks. Self-contained
        // -> the batch parallelises perfectly and the result is cached on the chunk;
        // meshing (and re-meshing) just samples it. Only a block edit re-bakes.
        #[cfg(not(target_arch = "wasm32"))]
        {
            use rayon::prelude::*;
            fresh.par_iter_mut().for_each(|(_, c)| {
                let (band, ylo, yhi) = crate::mesh::compute_chunk_skylight(c);
                c.set_skylight(band, ylo, yhi);
            });
        }
        #[cfg(target_arch = "wasm32")]
        for (_, c) in fresh.iter_mut() {
            let (band, ylo, yhi) = crate::mesh::compute_chunk_skylight(c);
            c.set_skylight(band, ylo, yhi);
        }

        let n = fresh.len();
        let mut ingested: Vec<ChunkPos> = Vec::with_capacity(n);
        for (pos, chunk) in fresh {
            self.chunks.insert(pos, chunk);
            ingested.push(pos);
        }

        // Mark the surrounding 3x3 dirty so they re-MESH against the new terrain:
        // the 4 cardinals for cross-chunk face culling, the full 3x3 so border faces
        // re-sample the new chunk's edge light. This is a cheap re-mesh only — those
        // neighbours' own cached skylight is unchanged (it's self-contained), so the
        // expensive flood-fill does NOT re-run for them.
        for pos in &ingested {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    if dx == 0 && dz == 0 { continue; }
                    let np = ChunkPos::new(pos.cx + dx, pos.cz + dz);
                    if let Some(c) = self.chunks.get_mut(&np) {
                        c.dirty = true;
                    }
                }
            }
        }
        n
    }

    /// Build meshes for chunks marked dirty (or pending mesh), limited by
    /// a per-frame budget so we don't stall.
    pub fn tick_mesh_budget(&mut self, max_per_frame: usize) {
        // Determine chunks that have neighbours loaded (horizontally).
        // We require all 4 horizontal neighbours present so cross-chunk
        // face culling is correct. If a neighbour is missing we still mesh
        // but with a permissive "assume air" fallback for outside.
        let to_mesh: Vec<ChunkPos> = self.chunks.iter()
            .filter(|(_, c)| c.dirty)
            .map(|(pos, _)| *pos)
            .take(max_per_frame)
            .collect();
        if to_mesh.is_empty() { return; }

        // Safety net that makes `light_dirty` the single source of truth for the
        // skylight cache: never mesh a chunk from stale light. `poll` (ingest) and
        // `set_block_world` (edit) already re-bake eagerly so this is normally a
        // no-op, but it guarantees correctness for ANY block-mutation path that
        // forgets to — it just has to leave the chunk dirty + light_dirty.
        for &pos in &to_mesh {
            if let Some(c) = self.chunks.get_mut(&pos) {
                if c.light_dirty {
                    let (band, ylo, yhi) = crate::mesh::compute_chunk_skylight(c);
                    c.set_skylight(band, ylo, yhi);
                }
            }
        }

        // Mesh building is a PURE function of (chunk, neighbour block/biome reads)
        // over an IMMUTABLE &self.chunks borrow — so every chunk can be meshed on a
        // separate thread and the resulting ChunkMesh is byte-identical to the
        // serial build (no shared mutable state). We collect (pos, mesh) pairs,
        // then flip dirty flags + insert meshes serially afterward (the only
        // mutation), keeping the immutable/mutable borrows disjoint in time.
        let chunks = &self.chunks;
        let build_one = move |pos: ChunkPos| -> Option<(ChunkPos, crate::mesh::ChunkMesh)> {
            let chunk = chunks.get(&pos)?;
            // Gather the 3x3 neighbourhood ONCE (indexed [(dz+1)*3 + (dx+1)]) so the
            // skylight apron + face-cull lookups index an array instead of hashing
            // the chunk HashMap per voxel. The skylight apron (LIGHT_R=16 = one
            // chunk) and cull (±1 block) never read past this 3x3, so the hash
            // fallback below is just belt-and-braces.
            let neigh: [Option<&Chunk>; 9] = std::array::from_fn(|k| {
                let dx = (k % 3) as i32 - 1;
                let dz = (k / 3) as i32 - 1;
                chunks.get(&ChunkPos::new(pos.cx + dx, pos.cz + dz))
            });
            // Resolve the chunk owning world chunk-coords (nx, nz) without hashing
            // when it's in the 3x3, else fall back to the map.
            let owner = move |nx: i32, nz: i32| -> Option<&Chunk> {
                let dcx = nx - pos.cx;
                let dcz = nz - pos.cz;
                if (-1..=1).contains(&dcx) && (-1..=1).contains(&dcz) {
                    neigh[((dcz + 1) * 3 + (dcx + 1)) as usize]
                } else {
                    chunks.get(&ChunkPos::new(nx, nz))
                }
            };
            let nb = |wx: i32, wy: i32, wz: i32| -> u8 {
                if wy < 0 || wy >= CHUNK_SY as i32 { return 0; }
                match owner(wx >> 4, wz >> 4) {
                    Some(c) => c.block_raw((wx & 0x0F) as usize, wy as usize, (wz & 0x0F) as usize),
                    None => 0,
                }
            };
            let nb_biome = |wx: i32, wz: i32| -> u8 {
                match owner(wx >> 4, wz >> 4) {
                    Some(c) => c.biome_at((wx & 0x0F) as usize, (wz & 0x0F) as usize),
                    None => 0,
                }
            };
            // Cached skylight (x2 scale) at a world voxel, routed to the owning
            // chunk's stored band. Above the world / unloaded neighbours read as
            // open sky; below the world as dark. Meshing only SAMPLES this — the
            // flood-fill already ran once when each chunk was ingested/edited.
            let nb_light = |wx: i32, wy: i32, wz: i32| -> u8 {
                if wy < 0 { return 0; }
                if wy >= CHUNK_SY as i32 { return SKY_FULL; }
                match owner(wx >> 4, wz >> 4) {
                    Some(c) => c.skylight_at((wx & 0x0F) as usize, wy, (wz & 0x0F) as usize),
                    None => SKY_FULL,
                }
            };
            Some((pos, build_mesh(chunk, nb, nb_biome, nb_light)))
        };

        #[cfg(not(target_arch = "wasm32"))]
        let built: Vec<(ChunkPos, crate::mesh::ChunkMesh)> = {
            use rayon::prelude::*;
            to_mesh.into_par_iter().filter_map(build_one).collect()
        };
        #[cfg(target_arch = "wasm32")]
        let built: Vec<(ChunkPos, crate::mesh::ChunkMesh)> =
            to_mesh.into_iter().filter_map(build_one).collect();

        for (pos, mesh) in built {
            self.meshes.insert(pos, mesh);
            if let Some(c) = self.chunks.get_mut(&pos) {
                c.dirty = false;
            }
        }
    }

    /// Iterate loaded chunk meshes for rendering (caller culls by camera).
    pub fn iter_meshes(&self) -> impl Iterator<Item = (ChunkPos, &ChunkMesh)> {
        self.meshes.iter().map(|(p, m)| (*p, m))
    }

    pub fn chunk_block(&self, wx: i32, wy: i32, wz: i32) -> u8 {
        if wy < 0 || wy >= CHUNK_SY as i32 { return 0; }
        let cx = wx >> 4; let cz = wz >> 4;
        let lx = (wx & 0x0F) as usize;
        let lz = (wz & 0x0F) as usize;
        if let Some(c) = self.chunks.get(&ChunkPos::new(cx, cz)) {
            c.block_raw(lx, wy as usize, lz)
        } else { 0 }
    }

    /// Is the chunk at chunk-coords `(cx, cz)` loaded?
    pub fn chunk_loaded(&self, cx: i32, cz: i32) -> bool {
        self.chunks.contains_key(&ChunkPos::new(cx, cz))
    }

    /// Set a block at world coords. Re-bakes the owning chunk's skylight and marks
    /// it + its full 3x3 neighbourhood dirty so the next `tick_mesh_budget` rebuilds
    /// them — the 3x3 so border faces re-sample the edited chunk's edge light and
    /// for cross-chunk face culling. Returns false if the chunk isn't loaded or `wy`
    /// is out of range. In-memory only.
    pub fn set_block_world(&mut self, wx: i32, wy: i32, wz: i32, b: Block) -> bool {
        if wy < 0 || wy >= CHUNK_SY as i32 { return false; }
        let cx = wx >> 4;
        let cz = wz >> 4;
        let lx = (wx & 0x0F) as usize;
        let lz = (wz & 0x0F) as usize;
        let Some(c) = self.chunks.get_mut(&ChunkPos::new(cx, cz)) else { return false; };
        c.set_block(lx, wy as usize, lz, b);
        // Re-bake THIS chunk's skylight (its blocks changed); neighbours keep their
        // cached light (self-contained) and only re-mesh to re-sample the new edge.
        let (band, ylo, yhi) = crate::mesh::compute_chunk_skylight(c);
        c.set_skylight(band, ylo, yhi);
        // Re-mesh the 3x3 so border faces re-sample this chunk's changed edge light
        // (and for cross-chunk face culling). Cheap re-mesh only — neighbours keep
        // their own cached skylight. A future optimisation could scope this to edits
        // near a chunk edge.
        for dz in -1..=1 {
            for dx in -1..=1 {
                if dx == 0 && dz == 0 { continue; }
                self.mark_dirty(cx + dx, cz + dz);
            }
        }
        true
    }

    fn mark_dirty(&mut self, cx: i32, cz: i32) {
        if let Some(n) = self.chunks.get_mut(&ChunkPos::new(cx, cz)) {
            n.dirty = true;
        }
    }
}