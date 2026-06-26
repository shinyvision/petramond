use std::collections::HashSet;

use crate::chunk::{Chunk, ChunkPos, CHUNK_SY, SKY_FULL};
use crate::mesh::build_mesh_lods_with_loaded_neighbors;

use super::store::World;

/// Set of chunks awaiting a remesh. With `World`'s chunk map private, every path
/// that dirties a chunk pushes here (`mark_dirty_pos`, `queue_dirty_mesh`, the
/// light-bake drain) and `remove_chunk` pulls it back out — so the set alone says
/// what needs meshing; there is no chunk-flag scan to reconcile against. Drained
/// NEAREST-FIRST to the load centre (see [`pop_nearest_batch`](Self::pop_nearest_batch))
/// so the terrain around the player meshes before the edges.
#[derive(Default)]
pub(super) struct DirtyMeshQueue {
    pending: HashSet<ChunkPos>,
}

impl DirtyMeshQueue {
    pub fn push(&mut self, pos: ChunkPos) {
        self.pending.insert(pos);
    }

    pub fn remove(&mut self, pos: ChunkPos) {
        self.pending.remove(&pos);
    }

    /// Pop up to `max` chunks, those nearest `center` (the player's chunk) first,
    /// so nearby terrain meshes before distant terrain. Meshing is idempotent (a
    /// rebuild from current state), so the order is a priority, not a contract — a
    /// popped chunk still blocked on light is simply re-pushed by the caller.
    /// `center` is `None` only before the first load target, where any order is fine.
    fn pop_nearest_batch(&mut self, max: usize, center: Option<ChunkPos>) -> Vec<ChunkPos> {
        if max == 0 || self.pending.is_empty() {
            return Vec::new();
        }
        let mut all: Vec<ChunkPos> = self.pending.iter().copied().collect();
        if let Some(c) = center {
            all.sort_by_key(|p| {
                let dx = (p.cx - c.cx) as i64;
                let dz = (p.cz - c.cz) as i64;
                dx * dx + dz * dz
            });
        }
        all.truncate(max);
        for pos in &all {
            self.pending.remove(pos);
        }
        all
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

        // Mesh nearest the player first; the load target's centre is the player's
        // chunk (`None` only before the first stream, where order doesn't matter).
        let center = self.last_load_target.as_ref().map(|t| t.center);
        let candidates = self.dirty_meshes.pop_nearest_batch(max_per_frame, center);
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
                    Some(c) => {
                        c.water_meta((wx & 0x0F) as usize, wy as usize, (wz & 0x0F) as usize)
                    }
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
                    chunk,
                    nb,
                    nb_water,
                    nb_biome,
                    nb_light,
                    nb_blocklight,
                    nb_loaded,
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

    /// The same blocks as [`fresh_chunk`], but reconstructed through
    /// [`Chunk::from_saved`] — the path a chunk read back from disk takes. The
    /// block bytes are taken from a real chunk so the in-array layout is exact.
    fn disk_chunk(cx: i32, cz: i32) -> Chunk {
        let template = fresh_chunk(cx, cz);
        let blocks: Box<[u8]> = Box::from(template.blocks_slice());
        let biomes: Vec<u8> = template.biomes_slice().to_vec();
        Chunk::from_saved(
            cx,
            cz,
            blocks,
            &biomes,
            None,
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
            Default::default(),
        )
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

    /// Repro probe for "disk-loaded chunks don't render": a chunk reconstructed
    /// via `Chunk::from_saved` (the codec decode path) and installed the way the
    /// streamer installs it must mesh identically to the generated chunk it was
    /// saved from — and remesh on a later edit.
    #[test]
    fn disk_loaded_chunk_meshes_like_a_generated_one() {
        let pos = ChunkPos::new(0, 0);

        let mut generated = World::new(0, 0);
        generated.insert_chunk_for_test(pos, fresh_chunk(pos.cx, pos.cz));
        tick_until_settled(&mut generated, pos);
        let generated_len = generated.meshes.get(&pos).unwrap().opaque_idx.len();
        assert!(generated_len > 0, "baseline generated chunk meshes");

        let mut disk = World::new(0, 0);
        disk.insert_chunk_for_test(pos, disk_chunk(pos.cx, pos.cz));
        tick_until_settled(&mut disk, pos);
        let disk_len = disk.meshes.get(&pos).unwrap().opaque_idx.len();

        assert_eq!(
            disk_len, generated_len,
            "disk-loaded chunk must mesh like the generated chunk it was saved from"
        );

        // And an edit must remesh it.
        assert!(disk.set_block_world(1, 2, 1, Block::Stone));
        tick_until_settled(&mut disk, pos);
        assert!(
            disk.meshes.get(&pos).unwrap().opaque_idx.len() > disk_len,
            "edit on a disk-loaded chunk must rebuild its mesh"
        );
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
