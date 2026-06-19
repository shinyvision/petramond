use crate::chunk::{Chunk, ChunkPos};
use crate::worker::GenRequest;

use super::store::{LoadTarget, World};

impl World {
    /// Update loaded chunk set around camera (in chunk coords).
    ///
    /// Expensive request and unload scans are gated to camera chunk-center (or
    /// render-distance) changes. Call `poll` every frame to keep ingesting worker
    /// results.
    pub fn update_load(&mut self, cam_chunk_x: i32, cam_chunk_z: i32) {
        let target = LoadTarget::new(cam_chunk_x, cam_chunk_z, self.render_dist);
        if self.last_load_target == Some(target) {
            return;
        }
        self.last_load_target = Some(target);

        self.request_missing_chunks(target.center, target.render_dist);
        self.unload_far_chunks(target.center, target.render_dist);
    }

    fn request_missing_chunks(&mut self, center: ChunkPos, r: i32) {
        // Request all chunks within radius (Euclidean approximation via squared).
        for dz in -r..=r {
            for dx in -r..=r {
                if dx * dx + dz * dz > r * r {
                    continue;
                }
                let pos = ChunkPos::new(center.cx + dx, center.cz + dz);
                if self.chunks.contains_key(&pos) {
                    continue;
                }
                if self.pending.contains_key(&pos) {
                    continue;
                }
                self.worker.submit(GenRequest {
                    cx: pos.cx,
                    cz: pos.cz,
                    seed: self.seed,
                });
                self.pending.insert(pos, ());
            }
        }
    }

    fn unload_far_chunks(&mut self, center: ChunkPos, r: i32) {
        let keep = r + 2;
        let to_drop: Vec<ChunkPos> = self
            .chunks
            .keys()
            .filter(|p| (p.cx - center.cx).abs() > keep || (p.cz - center.cz).abs() > keep)
            .cloned()
            .collect();
        for pos in to_drop {
            self.remove_chunk(pos);
        }
    }

    fn within_current_keep_radius(&self, pos: ChunkPos) -> bool {
        let Some(target) = self.last_load_target else {
            return true;
        };
        let keep = target.render_dist + 2;
        (pos.cx - target.center.cx).abs() <= keep && (pos.cz - target.center.cz).abs() <= keep
    }

    /// Poll worker for finished chunks and ingest.
    /// Returns number of chunks ingested.
    pub fn poll(&mut self) -> usize {
        let mut fresh: Vec<(ChunkPos, Chunk)> = Vec::new();
        while let Some(res) = self.worker.try_recv() {
            let pos = ChunkPos::new(res.cx, res.cz);
            self.pending.remove(&pos);
            if !self.within_current_keep_radius(pos) {
                continue;
            }
            fresh.push((pos, res.chunk));
        }
        if fresh.is_empty() {
            return 0;
        }

        // Bake each fresh chunk's skylight once from its own blocks. Meshing and
        // re-meshing only sample this cached band until a block edit re-bakes it.
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
            self.queue_dirty_mesh(pos);
            ingested.push(pos);
        }

        // Mark the surrounding 3x3 dirty so neighbours re-mesh against the new
        // terrain and edge light. Their cached skylight remains self-contained.
        for pos in &ingested {
            self.mark_dirty_neighborhood(*pos, false);
        }
        n
    }
}
