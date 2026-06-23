use std::collections::{HashSet, VecDeque};

use crate::chunk::{Chunk, ChunkPos, CHUNK_SY, SKY_FULL};
use crate::mesh::build_mesh_lods_with_loaded_neighbors;

use super::store::World;

/// FIFO of chunks awaiting a remesh, deduplicated. With `World`'s chunk map
/// private, every path that dirties a chunk pushes here (`mark_dirty_pos`,
/// `queue_dirty_mesh`, the light-bake drain) and `remove_chunk` pulls it back
/// out — so the queue alone says what needs meshing; there is no chunk-flag
/// scan to reconcile against.
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

    /// Pop up to `max` queued positions in FIFO order, skipping any that were
    /// removed (e.g. their chunk unloaded) since being enqueued.
    fn pop_batch(&mut self, max: usize) -> Vec<ChunkPos> {
        let mut out = Vec::with_capacity(max);
        while out.len() < max {
            let Some(pos) = self.order.pop_front() else {
                break;
            };
            if self.queued.remove(&pos) {
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

        self.drain_finished_light_bakes();

        let candidates = self.dirty_meshes.pop_batch(max_per_frame);
        if candidates.is_empty() {
            return;
        }

        // Safety net that makes `light_dirty` the single source of truth for the
        // skylight cache: never mesh a chunk from stale light, and never sample a
        // stale neighbor light band while building its border vertices. Dirty
        // light now bakes off-thread, so blocked mesh candidates are requeued.
        let mut to_mesh = Vec::with_capacity(candidates.len());
        for pos in candidates {
            if self.request_light_dependencies(pos) {
                self.dirty_meshes.push(pos);
            } else {
                to_mesh.push(pos);
            }
        }
        if to_mesh.is_empty() {
            return;
        }

        for &pos in &to_mesh {
            self.invalidate_section_visibility(pos);
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
            let nb_water = |wx: i32, wy: i32, wz: i32| -> u8 {
                if wy < 0 || wy >= CHUNK_SY as i32 {
                    return 0;
                }
                match owner(wx >> 4, wz >> 4) {
                    Some(c) => c.water_meta((wx & 0x0F) as usize, wy as usize, (wz & 0x0F) as usize),
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
            // Block-light (torches) reads 0 outside any chunk's band and above/below
            // the world — there is no block light without an emitter.
            let nb_blocklight = |wx: i32, wy: i32, wz: i32| -> u8 {
                if wy < 0 || wy >= CHUNK_SY as i32 {
                    return 0;
                }
                match owner(wx >> 4, wz >> 4) {
                    Some(c) => c.blocklight_at((wx & 0x0F) as usize, wy, (wz & 0x0F) as usize),
                    None => 0,
                }
            };
            let nb_loaded = |cx: i32, cz: i32| -> bool { owner(cx, cz).is_some() };
            Some((
                pos,
                build_mesh_lods_with_loaded_neighbors(
                    chunk, nb, nb_water, nb_biome, nb_light, nb_blocklight, nb_loaded,
                ),
            ))
        };

        let built: Vec<(ChunkPos, crate::mesh::ChunkMesh)> = {
            use rayon::prelude::*;
            to_mesh.into_par_iter().filter_map(build_one).collect()
        };

        for (pos, mesh) in built {
            self.meshes.insert(pos, mesh);
            if let Some(c) = self.chunks.get_mut(&pos) {
                c.dirty = false;
            }
        }
    }

    fn drain_finished_light_bakes(&mut self) {
        while let Some(res) = self.light_bakes.try_recv() {
            let fresh = self
                .chunks
                .get(&res.pos)
                .is_some_and(|c| c.light_dirty && c.light_revision == res.revision);
            if !fresh {
                continue;
            }
            if let Some(c) = self.chunks.get_mut(&res.pos) {
                c.set_skylight(res.band, res.ylo, res.yhi);
                c.set_blocklight(res.block_band, res.block_ylo, res.block_yhi);
                c.dirty = true;
            }
            self.bump_lighting_revision();
            self.dirty_meshes.push(res.pos);
        }
    }

    /// Queue every dirty light band a mesh would read from its 3x3 sampling
    /// neighbourhood. Returns true when the mesh must wait for async light.
    fn request_light_dependencies(&mut self, pos: ChunkPos) -> bool {
        let mut light_to_bake = Vec::new();
        for dz in -1..=1 {
            for dx in -1..=1 {
                let p = ChunkPos::new(pos.cx + dx, pos.cz + dz);
                if self.chunks.get(&p).is_some_and(|c| c.light_dirty) {
                    light_to_bake.push(p);
                }
            }
        }
        if light_to_bake.is_empty() {
            return false;
        }
        light_to_bake.sort_by_key(|p| (p.cz, p.cx));
        light_to_bake.dedup();
        for p in light_to_bake {
            self.light_bakes.request(p, &self.chunks);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::block::Block;
    use crate::mesh::compute_chunk_skylight;

    /// A settled chunk: skylight baked from its current blocks, flags clear.
    /// All-air, so it meshes empty until something is placed in it.
    fn clean_chunk(cx: i32, cz: i32) -> Chunk {
        let mut chunk = Chunk::new(cx, cz);
        let (band, ylo, yhi) = compute_chunk_skylight(&chunk);
        chunk.set_skylight(band, ylo, yhi);
        chunk.dirty = false;
        chunk
    }

    /// A freshly generated chunk as the streamer would hand it over: one solid
    /// block, light not yet baked (`dirty` + `light_dirty` set).
    fn fresh_chunk(cx: i32, cz: i32) -> Chunk {
        let mut chunk = Chunk::new(cx, cz);
        chunk.set_block(1, 1, 1, Block::Stone);
        chunk
    }

    /// Tick the mesh budget until `pos` settles (mesh + light flags clear),
    /// driving the async light bake to completion.
    fn tick_until_settled(world: &mut World, pos: ChunkPos) {
        for _ in 0..200 {
            world.tick_mesh_budget(1);
            if world
                .chunks
                .get(&pos)
                .is_some_and(|c| !c.dirty && !c.light_dirty)
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("chunk did not settle after async light bake");
    }

    /// The queue is the single source of truth: a freshly installed chunk
    /// (enqueued the way the streamer does) bakes its light and meshes — no
    /// chunk-flag scan reconciles it.
    #[test]
    fn installed_chunk_is_meshed_via_the_queue() {
        let pos = ChunkPos::new(0, 0);
        let mut world = World::new(0, 0);
        // `insert_chunk_for_test` mirrors the streamer's per-chunk install,
        // enqueueing the chunk for meshing.
        world.insert_chunk_for_test(pos, fresh_chunk(pos.cx, pos.cz));

        tick_until_settled(&mut world, pos);

        assert!(!world.meshes.get(&pos).unwrap().is_empty());
        assert!(world.lighting_revision() > 0);
    }

    /// An edit through the proper API (`set_block_world`) re-dirties and
    /// remeshes the chunk without any chunk-flag scan to reconcile it.
    #[test]
    fn set_block_world_edit_remeshes_the_chunk() {
        let pos = ChunkPos::new(0, 0);
        let mut world = World::new(0, 0);
        world.insert_chunk_for_test(pos, clean_chunk(pos.cx, pos.cz));
        tick_until_settled(&mut world, pos);
        assert!(world.meshes.get(&pos).unwrap().is_empty()); // all air so far
        let baked_before = world.lighting_revision();

        assert!(world.set_block_world(1, 1, 1, Block::Stone));
        assert!(world.chunks.get(&pos).unwrap().dirty);

        tick_until_settled(&mut world, pos);
        assert!(!world.meshes.get(&pos).unwrap().is_empty());
        assert!(world.lighting_revision() > baked_before);
    }
}
