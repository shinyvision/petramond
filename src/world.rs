//! World: manages loaded chunks, requests async generation, serves
//! neighbour-block queries for meshing.
//!
//! Gen is off-thread: see `worker` module.
//! We keep two maps in sync: a `loaded` map for terrain-only chunks and
//! `meshes` for baked meshes. The worldgen worker emits finished chunks
//! that the main thread ingests each frame.

use std::collections::HashMap;

use crate::block::Block;
use crate::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SY, CHUNK_SZ};
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
        let mut n = 0;
        let mut ingested: Vec<ChunkPos> = Vec::new();
        while let Some(res) = self.worker.try_recv() {
            let pos = ChunkPos::new(res.cx, res.cz);
            self.pending.remove(&pos);
            self.chunks.insert(pos, res.chunk);
            ingested.push(pos);
            n += 1;
        }
        // Mark horizontal neighbours dirty: a freshly-loaded chunk changes
        // the cross-chunk cull result for the 4 adjacent chunks' border
        // faces. Without remeshing them, borders built earlier against an
        // "assume air" fallback keep stale visible walls at chunk edges.
        for pos in &ingested {
            for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                let np = ChunkPos::new(pos.cx + dx, pos.cz + dz);
                if let Some(c) = self.chunks.get_mut(&np) {
                    c.dirty = true;
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

        // Mesh building is a PURE function of (chunk, neighbour block/biome reads)
        // over an IMMUTABLE &self.chunks borrow — so every chunk can be meshed on a
        // separate thread and the resulting ChunkMesh is byte-identical to the
        // serial build (no shared mutable state). We collect (pos, mesh) pairs,
        // then flip dirty flags + insert meshes serially afterward (the only
        // mutation), keeping the immutable/mutable borrows disjoint in time.
        let chunks = &self.chunks;
        let build_one = move |pos: ChunkPos| -> Option<(ChunkPos, crate::mesh::ChunkMesh)> {
            let chunk = chunks.get(&pos)?;
            let nb = |wx: i32, wy: i32, wz: i32| -> u8 {
                let nx = wx >> 4; let nz = wz >> 4;
                let lx = (wx & 0x0F) as usize;
                let lz = (wz & 0x0F) as usize;
                if wy < 0 || wy >= CHUNK_SY as i32 { return 0; }
                if nx == pos.cx && nz == pos.cz {
                    return chunk.block_raw(lx, wy as usize, lz);
                }
                if let Some(n) = chunks.get(&ChunkPos::new(nx, nz)) {
                    n.block_raw(lx, wy as usize, lz)
                } else { 0 }
            };
            let nb_biome = |wx: i32, wz: i32| -> u8 {
                let nx = wx >> 4; let nz = wz >> 4;
                let lx = (wx & 0x0F) as usize;
                let lz = (wz & 0x0F) as usize;
                if nx == pos.cx && nz == pos.cz {
                    return chunk.biome_at(lx, lz);
                }
                if let Some(n) = chunks.get(&ChunkPos::new(nx, nz)) {
                    n.biome_at(lx, lz)
                } else { 0 }
            };
            Some((pos, build_mesh(chunk, nb, nb_biome)))
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

    /// Set a block at world coords, marking the owning chunk (and any cardinal
    /// neighbour whose border faces this edit touches) dirty so the next
    /// `tick_mesh_budget` rebuilds them. Returns false if the chunk isn't loaded
    /// or `wy` is out of range. In-memory only — not persisted.
    pub fn set_block_world(&mut self, wx: i32, wy: i32, wz: i32, b: Block) -> bool {
        if wy < 0 || wy >= CHUNK_SY as i32 { return false; }
        let cx = wx >> 4;
        let cz = wz >> 4;
        let lx = (wx & 0x0F) as usize;
        let lz = (wz & 0x0F) as usize;
        let Some(c) = self.chunks.get_mut(&ChunkPos::new(cx, cz)) else { return false; };
        c.set_block(lx, wy as usize, lz, b);
        // A cell on a chunk edge changes the adjacent chunk's border-face cull.
        // Only cardinal neighbours matter (build_mesh reads cross-chunk faces
        // along ±X / ±Z only); a corner edit dirties both cardinal neighbours.
        if lx == 0 { self.mark_dirty(cx - 1, cz); }
        if lx == CHUNK_SX - 1 { self.mark_dirty(cx + 1, cz); }
        if lz == 0 { self.mark_dirty(cx, cz - 1); }
        if lz == CHUNK_SZ - 1 { self.mark_dirty(cx, cz + 1); }
        true
    }

    fn mark_dirty(&mut self, cx: i32, cz: i32) {
        if let Some(n) = self.chunks.get_mut(&ChunkPos::new(cx, cz)) {
            n.dirty = true;
        }
    }
}