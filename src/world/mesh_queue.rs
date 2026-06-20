use std::collections::{HashMap, HashSet, VecDeque};

use crate::chunk::{Chunk, ChunkPos, CHUNK_SY, SKY_FULL};
use crate::mesh::build_mesh_lods_with_loaded_neighbors;

use super::store::World;

#[derive(Default)]
pub(super) struct DirtyMeshQueue {
    order: VecDeque<ChunkPos>,
    queued: HashSet<ChunkPos>,
}

impl DirtyMeshQueue {
    pub fn push(&mut self, pos: ChunkPos) {
        if self.queued.insert(pos) {
            self.order.push_back(pos);
        }
    }

    pub fn remove(&mut self, pos: ChunkPos) {
        self.queued.remove(&pos);
    }

    fn enqueue_loaded_dirty(&mut self, chunks: &HashMap<ChunkPos, Chunk>) {
        for (&pos, chunk) in chunks {
            if chunk.dirty {
                self.push(pos);
            }
        }
    }

    fn pop_dirty(&mut self, chunks: &HashMap<ChunkPos, Chunk>, max: usize) -> Vec<ChunkPos> {
        let mut out = Vec::with_capacity(max);
        while out.len() < max {
            let Some(pos) = self.order.pop_front() else {
                break;
            };
            if !self.queued.remove(&pos) {
                continue;
            }
            if chunks.get(&pos).is_some_and(|c| c.dirty) {
                out.push(pos);
            }
        }
        out
    }
}

impl World {
    /// Build meshes for chunks queued dirty, limited by a per-frame budget so we
    /// do not stall the frame.
    pub fn tick_mesh_budget(&mut self, max_per_frame: usize) {
        if max_per_frame == 0 {
            return;
        }

        // `World::chunks` is public for compatibility, so callers can still
        // mutate chunks directly. Reconcile the private queue with the public
        // dirty flag before consuming it so those edits keep the old scan-based
        // semantics while queued writes remain deduplicated.
        self.dirty_meshes.enqueue_loaded_dirty(&self.chunks);

        let to_mesh = self.dirty_meshes.pop_dirty(&self.chunks, max_per_frame);
        if to_mesh.is_empty() {
            return;
        }

        // Safety net that makes `light_dirty` the single source of truth for the
        // skylight cache: never mesh a chunk from stale light.
        for &pos in &to_mesh {
            if let Some(c) = self.chunks.get_mut(&pos) {
                if c.light_dirty {
                    let (band, ylo, yhi) = crate::mesh::compute_chunk_skylight(c);
                    c.set_skylight(band, ylo, yhi);
                }
            }
        }

        // Mesh building is a pure function of the chunk plus immutable neighbour
        // reads. Build in parallel on native, then publish meshes serially.
        let chunks = &self.chunks;
        let build_one = move |pos: ChunkPos| -> Option<(ChunkPos, crate::mesh::ChunkMesh)> {
            let chunk = chunks.get(&pos)?;
            let neigh: [Option<&Chunk>; 9] = std::array::from_fn(|k| {
                let dx = (k % 3) as i32 - 1;
                let dz = (k / 3) as i32 - 1;
                chunks.get(&ChunkPos::new(pos.cx + dx, pos.cz + dz))
            });
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
                if wy < 0 || wy >= CHUNK_SY as i32 {
                    return 0;
                }
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
            let nb_light = |wx: i32, wy: i32, wz: i32| -> u8 {
                if wy < 0 {
                    return 0;
                }
                if wy >= CHUNK_SY as i32 {
                    return SKY_FULL;
                }
                match owner(wx >> 4, wz >> 4) {
                    Some(c) => c.skylight_at((wx & 0x0F) as usize, wy, (wz & 0x0F) as usize),
                    None => SKY_FULL,
                }
            };
            let nb_loaded = |cx: i32, cz: i32| -> bool { owner(cx, cz).is_some() };
            Some((
                pos,
                build_mesh_lods_with_loaded_neighbors(chunk, nb, nb_biome, nb_light, nb_loaded),
            ))
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
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::block::Block;
    use crate::mesh::compute_chunk_skylight;

    fn clean_chunk(cx: i32, cz: i32) -> Chunk {
        let mut chunk = Chunk::new(cx, cz);
        let (band, ylo, yhi) = compute_chunk_skylight(&chunk);
        chunk.set_skylight(band, ylo, yhi);
        chunk.dirty = false;
        chunk
    }

    fn public_chunk_mutation_is_meshed(mutate: impl FnOnce(&mut Chunk)) {
        let pos = ChunkPos::new(0, 0);
        let mut world = World::new(0, 0);
        world.chunks.insert(pos, clean_chunk(pos.cx, pos.cz));

        mutate(world.chunks.get_mut(&pos).unwrap());
        assert!(world.chunks.get(&pos).unwrap().dirty);

        world.tick_mesh_budget(1);

        let chunk = world.chunks.get(&pos).unwrap();
        assert!(!chunk.dirty);
        assert!(!chunk.light_dirty);
        assert!(!world.meshes.get(&pos).unwrap().is_empty());
    }

    #[test]
    fn tick_mesh_budget_reconciles_public_set_block_mutation() {
        public_chunk_mutation_is_meshed(|chunk| {
            chunk.set_block(1, 1, 1, Block::Stone);
        });
    }

    #[test]
    fn tick_mesh_budget_reconciles_public_set_block_raw_mutation() {
        public_chunk_mutation_is_meshed(|chunk| {
            chunk.set_block_raw(1, 1, 1, Block::Stone.id());
        });
    }
}
