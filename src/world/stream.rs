use std::collections::HashSet;

use crate::block::Block;
use crate::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SY, CHUNK_SZ};
use crate::mathh::IVec3;
use crate::worker::GenRequest;

use super::store::{LoadTarget, World};

const CHUNK_LAYER: usize = CHUNK_SX * CHUNK_SZ;

#[inline]
fn block_index(x: usize, y: usize, z: usize) -> usize {
    y * CHUNK_LAYER + z * CHUNK_SX + x
}

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
        // Gather the missing chunks in the radius (Euclidean approximation via
        // squared), then request them NEAREST-FIRST so terrain around the player
        // streams in before the edges. The gen/load pools and the light pool all
        // pull jobs in submission order, so ordering the submissions orders the
        // whole load pipeline; the mesh queue then drains nearest-first too.
        let mut missing: Vec<(i32, ChunkPos)> = Vec::new();
        for dz in -r..=r {
            for dx in -r..=r {
                let d2 = dx * dx + dz * dz;
                if d2 > r * r {
                    continue;
                }
                let pos = ChunkPos::new(center.cx + dx, center.cz + dz);
                if self.chunks.contains_key(&pos) || self.pending.contains_key(&pos) {
                    continue;
                }
                missing.push((d2, pos));
            }
        }
        missing.sort_by_key(|(d2, _)| *d2);
        for (_, pos) in missing {
            // Prefer a saved (player-modified) chunk over regenerating it.
            if self.save.as_ref().is_some_and(|s| s.manifest_contains(pos)) {
                if let Some(save) = self.save.as_ref() {
                    save.request_load(pos);
                }
            } else {
                self.worker.submit(GenRequest {
                    cx: pos.cx,
                    cz: pos.cz,
                    seed: self.seed,
                });
            }
            self.pending.insert(pos, ());
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
        // Persist any player-modified chunk, and any chunk holding item entities or
        // mobs, before it leaves memory. Unload's harvest policy DRAINS the drops and
        // mobs into the save record: that pauses item lifetime timers (they stop being
        // simulated) and takes the mobs out of the live set until the chunk loads again.
        // The gate + snapshot build itself is shared with the autosave flush
        // (`snapshot_chunk_for_save`).
        if self.save.is_some() {
            let mut snaps = Vec::new();
            for &pos in &to_drop {
                let entities = self.dropped_items.take_items_in_chunk(pos);
                let mobs = self.mobs.take_in_chunk(pos);
                let record_holds_entities = self
                    .save
                    .as_ref()
                    .is_some_and(|s| s.record_holds_entities(pos));
                if let Some(snap) =
                    self.snapshot_chunk_for_save(pos, entities, mobs, record_holds_entities)
                {
                    snaps.push(snap);
                }
            }
            if let Some(save) = self.save.as_mut() {
                save.save_chunks(snaps);
            }
        }
        // Post-action: evict each chunk now that its state is on the save queue.
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
        // Drain chunks read back from disk. A missing/corrupt record falls back
        // to generation so the player still sees terrain. Item entities stored in
        // the record rejoin the active set (their paused timers resume).
        while let Some(loaded) = self.save.as_ref().and_then(|s| s.poll_loaded()) {
            let pos = loaded.pos;
            self.pending.remove(&pos);
            if !self.within_current_keep_radius(pos) {
                continue;
            }
            match loaded.chunk {
                Some(chunk) => {
                    // The record carried drops or mobs: remember that, so a later
                    // flush that finds this chunk free of them rewrites the record
                    // instead of leaving stale entities to resurrect (cross-session
                    // dupe). Mobs rejoin the live set; drops resume their lifetimes.
                    if !loaded.entities.is_empty() || !loaded.mobs.is_empty() {
                        if let Some(save) = self.save.as_mut() {
                            save.note_record_holds_entities(pos);
                        }
                    }
                    self.dropped_items.extend(loaded.entities);
                    self.mobs.restore(loaded.mobs);
                    fresh.push((pos, chunk));
                }
                None => {
                    self.worker.submit(GenRequest {
                        cx: pos.cx,
                        cz: pos.cz,
                        seed: self.seed,
                    });
                    self.pending.insert(pos, ());
                }
            }
        }
        if fresh.is_empty() {
            return 0;
        }

        let n = fresh.len();
        let mut ingested: Vec<ChunkPos> = Vec::with_capacity(n);
        for (pos, chunk) in fresh {
            self.chunks.insert(pos, chunk);
            self.invalidate_section_visibility(pos);
            self.queue_dirty_mesh(pos);
            ingested.push(pos);
        }

        // Mark the surrounding 3x3 dirty so neighbors re-light and re-mesh
        // against the new terrain and border flood.
        for pos in &ingested {
            self.mark_light_dirty_neighborhood(*pos, true);
            self.mark_dirty_neighborhood(*pos, false);
        }
        self.queue_post_generation_block_updates(&ingested);
        n
    }

    /// Queue block updates for reactive generated blocks whose final loaded
    /// neighbourhood already says they need work. This runs after a batch is
    /// inserted so same-batch chunk borders are visible.
    ///
    /// These post the raw [`queue_block_update`](World::queue_block_update) rather
    /// than [`notify_block_and_neighbors`](World::notify_block_and_neighbors) — they
    /// only *kick* already-placed generated water into flowing, changing no block,
    /// and the relight that the announce would carry was already scheduled for the
    /// whole batch at ingest (the `mark_light_dirty_neighborhood` loop in `poll`).
    /// So they skip the per-update relight instead of redundantly re-dirtying every
    /// ingested chunk's 3×3.
    fn queue_post_generation_block_updates(&mut self, ingested: &[ChunkPos]) -> usize {
        let mut updates = Vec::new();
        let fresh: HashSet<ChunkPos> = ingested.iter().copied().collect();

        for &pos in ingested {
            self.collect_generated_water_updates(pos, &mut updates);
            self.collect_existing_neighbor_water_updates(pos, &fresh, &mut updates);
        }

        let mut queued = 0;
        for pos in updates {
            if self.queue_block_update(pos) {
                queued += 1;
            }
        }
        queued
    }

    /// Generated water starts as still source water. It only needs an initial
    /// update when the loaded neighbourhood gives it somewhere productive to go:
    /// down into air, or horizontally into air. Air above is just a normal water
    /// surface and does not cause flow.
    fn collect_generated_water_updates(&self, pos: ChunkPos, out: &mut Vec<IVec3>) {
        let Some(chunk) = self.chunks.get(&pos) else {
            return;
        };
        let blocks = chunk.blocks_slice();
        let air = Block::Air.id();
        let water = Block::Water.id();
        let west = self
            .chunks
            .get(&ChunkPos::new(pos.cx - 1, pos.cz))
            .map(Chunk::blocks_slice);
        let east = self
            .chunks
            .get(&ChunkPos::new(pos.cx + 1, pos.cz))
            .map(Chunk::blocks_slice);
        let north = self
            .chunks
            .get(&ChunkPos::new(pos.cx, pos.cz - 1))
            .map(Chunk::blocks_slice);
        let south = self
            .chunks
            .get(&ChunkPos::new(pos.cx, pos.cz + 1))
            .map(Chunk::blocks_slice);
        let (ox, oz) = chunk.chunk_origin_world();

        for y in 0..CHUNK_SY {
            let y_base = y * CHUNK_LAYER;
            for z in 0..CHUNK_SZ {
                let row = y_base + z * CHUNK_SX;
                for x in 0..CHUNK_SX {
                    let i = row + x;
                    if blocks[i] != water {
                        continue;
                    }

                    let needs_update = (y > 0 && blocks[i - CHUNK_LAYER] == air)
                        || if x > 0 {
                            blocks[i - 1] == air
                        } else {
                            west.is_some_and(|b| b[block_index(CHUNK_SX - 1, y, z)] == air)
                        }
                        || if x + 1 < CHUNK_SX {
                            blocks[i + 1] == air
                        } else {
                            east.is_some_and(|b| b[block_index(0, y, z)] == air)
                        }
                        || if z > 0 {
                            blocks[i - CHUNK_SX] == air
                        } else {
                            north.is_some_and(|b| b[block_index(x, y, CHUNK_SZ - 1)] == air)
                        }
                        || if z + 1 < CHUNK_SZ {
                            blocks[i + CHUNK_SX] == air
                        } else {
                            south.is_some_and(|b| b[block_index(x, y, 0)] == air)
                        };

                    if needs_update {
                        out.push(IVec3::new(ox + x as i32, y as i32, oz + z as i32));
                    }
                }
            }
        }
    }

    /// A chunk can arrive beside older source water that previously had no loaded
    /// target to flow into. Scan only the four shared border planes and wake that
    /// older water when the new chunk exposes air.
    fn collect_existing_neighbor_water_updates(
        &self,
        pos: ChunkPos,
        fresh: &HashSet<ChunkPos>,
        out: &mut Vec<IVec3>,
    ) {
        let Some(chunk) = self.chunks.get(&pos) else {
            return;
        };
        let blocks = chunk.blocks_slice();
        let air = Block::Air.id();
        let water = Block::Water.id();
        let (ox, oz) = chunk.chunk_origin_world();

        let west_pos = ChunkPos::new(pos.cx - 1, pos.cz);
        if !fresh.contains(&west_pos) {
            if let Some(neighbor) = self.chunks.get(&west_pos) {
                let nblocks = neighbor.blocks_slice();
                let wx = ox - 1;
                for y in 0..CHUNK_SY {
                    for z in 0..CHUNK_SZ {
                        if blocks[block_index(0, y, z)] == air
                            && nblocks[block_index(CHUNK_SX - 1, y, z)] == water
                        {
                            out.push(IVec3::new(wx, y as i32, oz + z as i32));
                        }
                    }
                }
            }
        }

        let east_pos = ChunkPos::new(pos.cx + 1, pos.cz);
        if !fresh.contains(&east_pos) {
            if let Some(neighbor) = self.chunks.get(&east_pos) {
                let nblocks = neighbor.blocks_slice();
                let wx = ox + CHUNK_SX as i32;
                for y in 0..CHUNK_SY {
                    for z in 0..CHUNK_SZ {
                        if blocks[block_index(CHUNK_SX - 1, y, z)] == air
                            && nblocks[block_index(0, y, z)] == water
                        {
                            out.push(IVec3::new(wx, y as i32, oz + z as i32));
                        }
                    }
                }
            }
        }

        let north_pos = ChunkPos::new(pos.cx, pos.cz - 1);
        if !fresh.contains(&north_pos) {
            if let Some(neighbor) = self.chunks.get(&north_pos) {
                let nblocks = neighbor.blocks_slice();
                let wz = oz - 1;
                for y in 0..CHUNK_SY {
                    for x in 0..CHUNK_SX {
                        if blocks[block_index(x, y, 0)] == air
                            && nblocks[block_index(x, y, CHUNK_SZ - 1)] == water
                        {
                            out.push(IVec3::new(ox + x as i32, y as i32, wz));
                        }
                    }
                }
            }
        }

        let south_pos = ChunkPos::new(pos.cx, pos.cz + 1);
        if !fresh.contains(&south_pos) {
            if let Some(neighbor) = self.chunks.get(&south_pos) {
                let nblocks = neighbor.blocks_slice();
                let wz = oz + CHUNK_SZ as i32;
                for y in 0..CHUNK_SY {
                    for x in 0..CHUNK_SX {
                        if blocks[block_index(x, y, CHUNK_SZ - 1)] == air
                            && nblocks[block_index(x, y, 0)] == water
                        {
                            out.push(IVec3::new(ox + x as i32, y as i32, wz));
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_chunk(cx: i32, cz: i32) -> Chunk {
        let mut chunk = Chunk::new(cx, cz);
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                chunk.set_block(x, 64, z, Block::Stone);
            }
        }
        chunk
    }

    fn run_ticks(world: &mut World, n: u32) {
        // These tests drive block updates / water flow only; no furnaces.
        let recipes = crate::crafting::Recipes::default();
        for _ in 0..n {
            world.game_tick(&recipes);
        }
    }

    fn block(world: &World, x: i32, y: i32, z: i32) -> Block {
        Block::from_id(world.chunk_block(x, y, z))
    }

    #[test]
    fn post_generation_updates_start_generated_water_flow() {
        let mut world = World::new(0, 0);
        let pos = ChunkPos::new(0, 0);
        let mut chunk = flat_chunk(pos.cx, pos.cz);
        chunk.set_block(8, 65, 8, Block::Water);
        world.chunks.insert(pos, chunk);

        assert_eq!(world.queue_post_generation_block_updates(&[pos]), 1);
        run_ticks(&mut world, super::super::water::WATER_FLOW_DELAY as u32 + 2);

        assert_eq!(block(&world, 9, 65, 8), Block::Water);
    }

    #[test]
    fn post_generation_updates_existing_neighbor_water_at_new_air_border() {
        let mut world = World::new(0, 0);
        let west_pos = ChunkPos::new(0, 0);
        let east_pos = ChunkPos::new(1, 0);
        let mut west = flat_chunk(west_pos.cx, west_pos.cz);
        west.set_block(CHUNK_SX - 1, 65, 8, Block::Water);

        world.chunks.insert(west_pos, west);
        world
            .chunks
            .insert(east_pos, flat_chunk(east_pos.cx, east_pos.cz));

        assert_eq!(world.queue_post_generation_block_updates(&[east_pos]), 1);
        run_ticks(&mut world, super::super::water::WATER_FLOW_DELAY as u32 + 2);

        assert_eq!(block(&world, 16, 65, 8), Block::Water);
    }

    #[test]
    fn post_generation_updates_ignore_water_with_only_air_above() {
        let mut world = World::new(0, 0);
        let pos = ChunkPos::new(0, 0);
        let mut chunk = flat_chunk(pos.cx, pos.cz);
        let p = (8, 65, 8);
        chunk.set_block(p.0, p.1, p.2, Block::Water);
        chunk.set_block(p.0 - 1, p.1, p.2, Block::Stone);
        chunk.set_block(p.0 + 1, p.1, p.2, Block::Stone);
        chunk.set_block(p.0, p.1, p.2 - 1, Block::Stone);
        chunk.set_block(p.0, p.1, p.2 + 1, Block::Stone);
        world.chunks.insert(pos, chunk);

        assert_eq!(world.queue_post_generation_block_updates(&[pos]), 0);
    }

    #[test]
    fn unloading_rewrites_a_chunk_whose_record_holds_a_picked_up_drop() {
        // The reported repro: a chunk saved with a drop, the drop since picked up,
        // must be re-saved on the next unload so the stale record can't resurrect
        // it on reload.
        let dir = std::env::temp_dir().join(format!(
            "llamacraft-streamtest-{}-unload-rewrite",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mut opened = crate::save::open_at(dir.clone()).expect("open temp world");

        let pos = ChunkPos::new(0, 0);
        // The save already holds a drop for this chunk (as a prior unload-with-item
        // left it); the chunk is back in memory now, drop-free and unmodified.
        opened.save.note_record_holds_entities(pos); // mirrors the load path
        let mut world = World::new(0, 1);
        world.attach_save(opened.save);
        world.chunks.insert(pos, Chunk::new(pos.cx, pos.cz));

        // Stream far away so the chunk unloads.
        world.unload_far_chunks(ChunkPos::new(1000, 1000), 1);

        assert!(
            !world.save().expect("save").record_holds_entities(pos),
            "unload must rewrite the chunk and clear its stale drop record"
        );

        drop(world); // join the save I/O thread before removing the dir
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// End-to-end repro for "chunks loaded from disk don't render geometry":
    /// modify a chunk, unload it (which saves it), then reload it from disk through
    /// the streamer's poll path and confirm it both builds a mesh and remeshes on a
    /// later edit — the block data survives either way (that's why collision works).
    #[test]
    fn modified_chunk_reloaded_from_disk_meshes_and_remeshes() {
        let dir = std::env::temp_dir().join(format!(
            "llamacraft-streamtest-{}-reload-mesh",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let opened = crate::save::open_at(dir.clone()).expect("open temp world");
        let mut world = World::new(0, 2);
        world.attach_save(opened.save);
        let pos = ChunkPos::new(0, 0);

        // Drive the mesh budget + async light bake (+ save I/O) until `cond`.
        fn settle(world: &mut World, cond: impl Fn(&World) -> bool) -> bool {
            for _ in 0..1000 {
                let _ = world.poll();
                world.tick_mesh_budget(64);
                if cond(world) {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            false
        }
        let mesh_len = |w: &World, p: ChunkPos| {
            w.iter_meshes()
                .find(|(mp, _)| *mp == p)
                .map(|(_, m)| m.opaque_idx.len())
        };

        // Install a chunk and edit it, so it is non-empty AND modified (persisted).
        world.insert_chunk_for_test(pos, flat_chunk(pos.cx, pos.cz));
        assert!(
            world.set_block_world(8, 90, 8, Block::Stone),
            "place marker"
        );
        assert!(
            settle(&mut world, |w| mesh_len(w, pos).is_some_and(|n| n > 0)),
            "chunk meshes before unload"
        );
        let before = mesh_len(&world, pos).unwrap();

        // Unload far away: the modified chunk is saved + evicted.
        world.unload_far_chunks(ChunkPos::new(1000, 1000), 1);
        assert!(
            !world.chunk_loaded(pos.cx, pos.cz),
            "chunk evicted on unload"
        );

        // Reload it from disk through the streamer's poll ingestion path.
        world.save().expect("save").request_load(pos);
        assert!(
            settle(&mut world, |w| w.chunk_loaded(pos.cx, pos.cz)),
            "chunk reloads from disk"
        );
        assert_eq!(
            world.chunk_block(8, 90, 8),
            Block::Stone.id(),
            "block data restored (so collision works)"
        );
        assert!(
            settle(&mut world, |w| mesh_len(w, pos).is_some_and(|n| n > 0)),
            "REPRO: reloaded chunk must build a non-empty mesh"
        );
        assert_eq!(
            mesh_len(&world, pos).unwrap(),
            before,
            "reloaded mesh matches the saved chunk's mesh"
        );

        // An edit on the reloaded chunk must rebuild its mesh.
        assert!(
            world.set_block_world(8, 90, 8, Block::Air),
            "mine the marker"
        );
        assert!(
            settle(&mut world, |w| w.chunk_block(8, 90, 8) == Block::Air.id()
                && mesh_len(w, pos).is_some_and(|n| n < before)),
            "REPRO: edit on the reloaded chunk must remesh (fewer faces after mining)"
        );

        drop(world);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// End-to-end mob persistence: a mob standing in a chunk is saved into that chunk's
    /// record when it unloads (leaving the live set), and comes back — same species,
    /// position and facing — when the chunk reloads from disk.
    #[test]
    fn a_mob_is_saved_on_unload_and_restored_on_reload() {
        use crate::mathh::Vec3;
        use crate::mob::Mob;

        let dir = std::env::temp_dir().join(format!(
            "llamacraft-streamtest-{}-mob-persist",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let opened = crate::save::open_at(dir.clone()).expect("open temp world");
        let mut world = World::new(0, 3);
        world.attach_save(opened.save);
        let pos = ChunkPos::new(0, 0);

        // A loaded chunk with an owl standing in it, facing a known yaw.
        world.insert_chunk_for_test(pos, flat_chunk(pos.cx, pos.cz));
        let feet = Vec3::new(8.5, 65.0, 8.5);
        assert!(world.mobs_mut().spawn(Mob::Owl, feet, 1.5));
        assert_eq!(world.mobs().len(), 1);

        // Stream far away: the owl's chunk unloads, harvesting it into the save record.
        world.unload_far_chunks(ChunkPos::new(1000, 1000), 1);
        assert!(
            !world.chunk_loaded(pos.cx, pos.cz),
            "chunk evicted on unload"
        );
        assert_eq!(
            world.mobs().len(),
            0,
            "the owl leaves the live set when its chunk unloads (it is saved, not simulated)"
        );

        // Reload the chunk from disk through the streamer's poll path: the owl returns.
        world.save().expect("save").request_load(pos);
        for _ in 0..1000 {
            let _ = world.poll();
            if world.chunk_loaded(pos.cx, pos.cz) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(
            world.chunk_loaded(pos.cx, pos.cz),
            "chunk reloads from disk"
        );
        assert_eq!(
            world.mobs().len(),
            1,
            "REPRO: the saved owl returns with its chunk"
        );
        let owl = &world.mobs().instances()[0];
        assert_eq!(owl.kind, Mob::Owl);
        assert_eq!(owl.pos, feet, "restored in place");
        assert_eq!(owl.yaw, 1.5, "facing restored");

        drop(world);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
