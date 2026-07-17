use crate::block::{Aabb, Block};
use crate::chunk::{self, ChunkPos, SectionPos, SKY_FULL, WORLD_MIN_Y};
use crate::mathh::IVec3;
use crate::mesh::ChunkMesh;
use crate::section::SectionSummary;

use super::store::World;

impl World {
    /// Iterate loaded section meshes for rendering (caller culls by camera).
    pub fn iter_meshes(&self) -> impl Iterator<Item = (SectionPos, &ChunkMesh)> {
        self.meshes.iter().map(|(p, m)| (*p, m))
    }

    /// Is any terrain CPU light/mesh work still queued or in flight? Tooling uses this
    /// to detect when the background pipeline has settled; renderer upload dirtiness is
    /// tracked separately because headless profilers have no renderer to clear it.
    pub fn has_dirty_meshes(&self) -> bool {
        !self.dirty_meshes.is_empty()
            || (self.role != super::store::WorldRole::ServerHeadless && self.vis_dirty)
            || !self.light_blocked_meshes.is_empty()
            || !self.light_deferred.is_empty()
            || self.light_bakes.has_pending()
            || self.prediction_terrain.has_pending()
            || self.mesh_jobs_in_flight > 0
    }

    /// Number of loaded sections — a diagnostic for streaming/perf tooling.
    pub fn loaded_section_count(&self) -> usize {
        self.sections.len()
    }

    /// Number of sections queued for (re)mesh — the streaming backlog.
    pub fn dirty_mesh_count(&self) -> usize {
        self.dirty_meshes.len() + self.light_blocked_meshes.len()
    }

    /// Number of loaded columns — a diagnostic for streaming/perf tooling.
    pub fn loaded_column_count(&self) -> usize {
        self.columns.len()
    }

    /// (deep, visible-deep, hidden-parked) counts — a visibility diagnostic for
    /// streaming/perf tooling.
    pub fn deep_visibility_counts(&self) -> (usize, usize, usize) {
        (
            self.deep_sections.len(),
            self.visible_deep.len(),
            self.hidden_parked.len(),
        )
    }

    /// Raw block id at a world voxel. Out of range (above/below the column) or in
    /// an unloaded chunk reads as `0` (air) — the mesh-border air fallback.
    pub fn chunk_block(&self, wx: i32, wy: i32, wz: i32) -> u8 {
        match self.chunk_at_world(wx, wy, wz) {
            Some((c, lx, ly, lz)) => c.block_raw(lx, ly, lz),
            None => 0,
        }
    }

    /// The block at a world voxel, or `None` if its chunk is not loaded or `wy` is
    /// outside the column. Unlike [`chunk_block`](Self::chunk_block) — which
    /// collapses both "unloaded" and "air" to `0` — this keeps them distinct, for
    /// callers that must NOT treat an unknown cell as air (leaf-decay support keeps
    /// a leaf whose neighbour is merely off the edge of what's loaded).
    pub fn block_if_loaded(&self, wx: i32, wy: i32, wz: i32) -> Option<Block> {
        let (c, lx, ly, lz) = self.chunk_at_world(wx, wy, wz)?;
        Some(c.block(lx, ly, lz))
    }

    /// Water-flow metadata at a world voxel (0 where the cell is not flowing
    /// water or its chunk is unloaded). See `world::water` for the encoding.
    pub fn water_meta_world(&self, wx: i32, wy: i32, wz: i32) -> u8 {
        match self.chunk_at_world(wx, wy, wz) {
            Some((c, lx, ly, lz)) => c.water_meta(lx, ly, lz),
            None => 0,
        }
    }

    /// Cached skylight at a world voxel on the x2 scale (`SKY_FULL` = light 15).
    /// Missing chunks read as open sky, matching mesh-border fallback behavior.
    pub fn skylight_at_world(&self, wx: i32, wy: i32, wz: i32) -> u8 {
        // Below the world floor is dark; above the world top and unloaded sections
        // read as open sky (the mesh-border fallback).
        if wy < WORLD_MIN_Y {
            return 0;
        }
        match self.chunk_at_world(wx, wy, wz) {
            Some((c, lx, ly, lz)) => c.skylight_at(lx, ly, lz),
            None => SKY_FULL,
        }
    }

    /// Cached skylight converted to the 6-bit packed vertex scale (`0..=63`).
    pub fn skylight6_at_world(&self, wx: i32, wy: i32, wz: i32) -> u8 {
        let l = self.skylight_at_world(wx, wy, wz) as u32;
        ((l * 63 + SKY_FULL as u32 / 2) / SKY_FULL as u32).min(63) as u8
    }

    /// Cached block-light (torches) at a world voxel on the x2 scale. `0` outside any
    /// chunk's block-light band and in unloaded chunks — there is no block light
    /// without a nearby emitter.
    pub fn blocklight_at_world(&self, wx: i32, wy: i32, wz: i32) -> u8 {
        match self.chunk_at_world(wx, wy, wz) {
            Some((c, lx, ly, lz)) => c.blocklight_at(lx, ly, lz),
            None => 0,
        }
    }

    /// Block-light converted to the 6-bit packed vertex scale (`0..=63`).
    pub fn blocklight6_at_world(&self, wx: i32, wy: i32, wz: i32) -> u8 {
        let l = self.blocklight_at_world(wx, wy, wz) as u32;
        ((l * 63 + SKY_FULL as u32 / 2) / SKY_FULL as u32).min(63) as u8
    }

    /// The brighter of skylight and block-light (6-bit) — how dynamic geometry (the
    /// held item / hand, particles, dropped items) should be lit, mirroring the way
    /// the chunk mesher folds the two channels so a torch lights them too.
    pub fn combined_light6_at_world(&self, wx: i32, wy: i32, wz: i32) -> u8 {
        self.skylight6_at_world(wx, wy, wz)
            .max(self.blocklight6_at_world(wx, wy, wz))
    }

    /// The TWO light channels plus the warm-tint amount for dynamic geometry (the
    /// held item / hand, mobs, dropped items, particles). Returns `(sky6, block6,
    /// warm)` — the channels stay separate so the renderer can dim/tint the sky
    /// term (day/night mods) without dimming torch light, mirroring the split
    /// terrain vertex; `warm` is `crate::torch::warm_amount * 255` packed into a
    /// byte (divide by 255 at render) so the same warmth the chunk mesher bakes
    /// into static blocks applies to dynamic geometry.
    pub fn dynamic_light_at_world(&self, wx: i32, wy: i32, wz: i32) -> (u8, u8, u8) {
        let sky6 = self.skylight6_at_world(wx, wy, wz);
        let block6 = self.blocklight6_at_world(wx, wy, wz);
        let warm = crate::torch::warm_amount(sky6 as f32 / 63.0, block6 as f32 / 63.0);
        (sky6, block6, (warm * 255.0) as u8)
    }

    /// Biome id for the loaded world column at `(wx, wz)`, or `None` if its
    /// owning chunk is not currently loaded.
    pub fn column_biome(&self, wx: i32, wz: i32) -> Option<u8> {
        self.columns
            .get(&ChunkPos::new(wx >> 4, wz >> 4))
            .map(|c| c.biome_at(chunk::lx(wx), chunk::lz(wz)))
    }

    /// Is any section of the column at chunk-coords `(cx, cz)` loaded?
    pub fn chunk_loaded(&self, cx: i32, cz: i32) -> bool {
        Self::column_section_range()
            .any(|cy| self.sections.contains_key(&SectionPos::new(cx, cy, cz)))
    }

    /// Whether a world cell can be built into: its column is loaded, it lies within the
    /// vertical range, and whatever occupies it is replaceable. The cubic replacement
    /// for the column era's `chunk_at_world(..).is_some() && replaceable` idiom — an
    /// all-air section is *absent* (the streamer skips it, and `chunk_block` reads it as
    /// air), yet a block may still be placed there (a tower, a door, a torch in mid-air).
    pub fn placement_cell_open(&self, c: IVec3) -> bool {
        if !self.chunk_loaded(c.x >> 4, c.z >> 4) {
            return false;
        }
        let Some(pos) = SectionPos::from_world(c.x, c.y, c.z) else {
            return false;
        };
        if self.sections.contains_key(&pos) {
            return Block::from_id(self.chunk_block(c.x, c.y, c.z)).is_replaceable();
        }
        match self.section_summary(pos) {
            SectionSummary::Empty => true,
            SectionSummary::FullWater => Block::Water.is_replaceable(),
            SectionSummary::Unknown => self.absent_cell_above_known_surface(c, pos),
            SectionSummary::FullOpaque | SectionSummary::Mixed => false,
        }
    }

    #[inline]
    fn absent_cell_above_known_surface(&self, c: IVec3, pos: SectionPos) -> bool {
        if self.saved_section_contains(pos) {
            return false;
        }
        self.columns
            .get(&pos.chunk_pos())
            .is_some_and(|col| c.y > col.surface_y(chunk::lx(c.x), chunk::lz(c.z)))
    }

    /// The loaded streaming disc — `(center_chunk_x, center_chunk_z, render_dist)`
    /// the chunk loader is currently centered on — or `None` before the first load.
    /// Natural mob spawning samples positions within this.
    pub fn loaded_area(&self) -> Option<(i32, i32, i32)> {
        self.last_load_target
            .map(|t| (t.center.cx, t.center.cz, t.render_dist))
    }

    /// The Y of the topmost movement-blocking block in the loaded column at
    /// `(wx, wz)` — the surface an entity's feet would rest on top of — or `None`
    /// if the chunk is unloaded or the column has no solid block. Non-colliding
    /// cover (tall grass, snow layers, water) is skipped, so the result is real
    /// footing rather than whatever happens to top the column.
    pub fn surface_collision_y(&self, wx: i32, wz: i32) -> Option<i32> {
        let col = self.columns.get(&ChunkPos::new(wx >> 4, wz >> 4))?;
        let top = col.surface_y(chunk::lx(wx), chunk::lz(wz));
        (WORLD_MIN_Y..=top)
            .rev()
            .find(|&y| self.blocks_movement_at(wx, y, wz))
    }

    /// The loaded column's biome id at world `(wx, wz)`, or `None` when the
    /// chunk is unloaded. Biomes are column-level data fixed at generation.
    pub fn biome_at_world(&self, wx: i32, wz: i32) -> Option<u8> {
        let col = self.columns.get(&ChunkPos::new(wx >> 4, wz >> 4))?;
        Some(col.biome_at(chunk::lx(wx) as usize, chunk::lz(wz) as usize))
    }

    /// The Y of the topmost cell precipitation lands on in the loaded column
    /// at `(wx, wz)` — the first movement-blocking OR water cell scanning
    /// down from the column top — or `None` if the chunk is unloaded or the
    /// column is all air. Walk-through cover (tall grass, snow layers,
    /// flowers) lets rain fall THROUGH to the ground under it, while a lake
    /// surface stops it like a roof does.
    pub fn precipitation_ceiling_y(&self, wx: i32, wz: i32) -> Option<i32> {
        let col = self.columns.get(&ChunkPos::new(wx >> 4, wz >> 4))?;
        let top = col.surface_y(chunk::lx(wx), chunk::lz(wz));
        (WORLD_MIN_Y..=top).rev().find(|&y| {
            self.blocks_movement_at(wx, y, wz)
                || self
                    .block_if_loaded(wx, y, wz)
                    .is_some_and(|b| b.is_water())
        })
    }

    /// Whether this loaded block is valid full-cube support for programmatic
    /// mob spawns. This is stricter than movement collision: leaves are solid
    /// for collision but remain canopy, and partial shapes such as stairs or
    /// model blocks are not full spawn footing.
    pub fn block_is_full_spawn_support(&self, wx: i32, wy: i32, wz: i32) -> bool {
        let Some(block) = self.block_if_loaded(wx, wy, wz) else {
            return false;
        };
        if block.is_water() || block.is_leaves() {
            return false;
        }
        full_unit_cube(self.collision_boxes_at(wx, wy, wz))
    }
}

fn full_unit_cube(boxes: &[Aabb]) -> bool {
    if boxes.len() != 1 {
        return false;
    }
    let b = boxes[0];
    b.min == [0.0, 0.0, 0.0] && b.max == [1.0, 1.0, 1.0]
}

#[cfg(test)]
mod tests {
    use crate::chunk::ChunkPos;

    use super::*;

    #[test]
    fn full_spawn_support_rejects_water_leaves_partials_and_unloaded_cells() {
        let mut world = World::new(0, 1);
        world.insert_empty_column_for_test(ChunkPos::new(0, 0));

        assert!(!world.block_is_full_spawn_support(8, 63, 8));

        assert!(world.set_block_world(8, 63, 8, Block::Grass));
        assert!(world.block_is_full_spawn_support(8, 63, 8));

        assert!(world.set_block_world(8, 63, 8, Block::Water));
        assert!(!world.block_is_full_spawn_support(8, 63, 8));

        assert!(world.set_block_world(8, 63, 8, Block::OakLeaves));
        assert!(!world.block_is_full_spawn_support(8, 63, 8));

        assert!(world.set_block_world(8, 63, 8, Block::OakStairs));
        assert!(!world.block_is_full_spawn_support(8, 63, 8));

        assert!(!world.block_is_full_spawn_support(128, 63, 128));
    }
}
