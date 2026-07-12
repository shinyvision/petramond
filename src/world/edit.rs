use crate::block::{Block, RenderShape};
use crate::block_state::LogAxis;
use crate::chunk::{ChunkPos, SECTION_SIZE, WORLD_MIN_Y};
use crate::column::NO_SURFACE;
use crate::mathh::IVec3;
use crate::section::SectionSummary;

use super::store::World;

impl World {
    /// Cells a player break at `pos` clears: door both halves, model footprint,
    /// or the single cell. Used by optimistic client clears and server
    /// corrective-cell sync so deny restores the full footprint.
    pub fn break_footprint_cells(&self, pos: IVec3) -> Vec<IVec3> {
        let block = Block::from_id(self.chunk_block(pos.x, pos.y, pos.z));
        match block.render_shape() {
            RenderShape::Model(_) => self
                .model_group(pos)
                .map(|(_, _, cells)| cells)
                .unwrap_or_else(|| vec![pos]),
            RenderShape::Door => self
                .door_cells(pos)
                .map(|(lower, upper)| vec![lower, upper])
                .unwrap_or_else(|| vec![pos]),
            _ => vec![pos],
        }
    }

    /// Snapshot previous block ids for [`break_footprint_cells`], then clear
    /// the footprint the same way the server's break funnel does (door /
    /// model / single air). No drops. Returns `(broken_block, cells_with_prev)`
    /// or `None` when the cell is already air / unbreakable.
    pub fn clear_broken_block(&mut self, pos: IVec3) -> Option<(Block, Vec<(IVec3, u8)>)> {
        let block = Block::from_id(self.chunk_block(pos.x, pos.y, pos.z));
        if block.hardness() < 0.0 {
            return None;
        }
        let cells: Vec<(IVec3, u8)> = self
            .break_footprint_cells(pos)
            .into_iter()
            .map(|c| (c, self.chunk_block(c.x, c.y, c.z)))
            .collect();
        match block.render_shape() {
            RenderShape::Model(_) => {
                let _ = self.remove_model_block(pos);
            }
            RenderShape::Door => {
                let _ = self.remove_door(pos);
            }
            _ => {
                let _ = self.set_block_world(pos.x, pos.y, pos.z, Block::Air);
            }
        }
        Some((block, cells))
    }

    /// Set a block at world coords. Updates the column surface heightmap, marks the
    /// owning section's light plus its full 3×3×3 neighbourhood dirty so the next
    /// `tick_mesh_budget` refreshes cached light and rebuilds meshes. Returns false
    /// if the section is not loaded or `wy` is out of range. In-memory only.
    pub fn set_block_world(&mut self, wx: i32, wy: i32, wz: i32, b: Block) -> bool {
        let Some((pos, lx, ly, lz)) = Self::split_world(wx, wy, wz) else {
            return false;
        };
        // Streaming-finality guard: a section whose gen result or saved overlay is
        // still in flight must not change — the landing result would clobber the
        // write, or the write would be persisted over the player's on-disk record
        // (see `world::sim_guard`). The blocked state resolves within a few frames.
        if !self.stream_writable(pos) {
            return false;
        }
        if !self.sections.contains_key(&pos) {
            // Building into absent sky materializes an empty section. Editing an absent
            // generated-solid/water section materializes its generated base first, so the
            // write changes one cell instead of replacing the whole section with air.
            let summary = self.section_summary(pos);
            let absent_air = matches!(summary, SectionSummary::Empty | SectionSummary::Unknown);
            if (b == Block::Air && absent_air) || !self.materialize_section(pos) {
                return false;
            }
        }
        {
            let Some(s) = self.section_mut(pos) else {
                return false;
            };
            s.set_block(lx, ly, lz, b);
            s.modified = true;
        }
        self.refresh_particle_emitter_index(pos);
        if self.update_column_height_after_set(wx, wy, wz, b != Block::Air) {
            self.mark_heightmap_light_dirty_around(pos.chunk_pos());
        }

        // Re-mesh the 3×3×3 so the border flood, vertex light sampling, and
        // cross-section face culling remain correct.
        self.mark_dirty_neighborhood(pos, true);
        // Plane openness may have changed; deep-visibility must re-evaluate.
        self.vis_dirty = true;

        // Announce the change: re-lights the neighbourhood and lets reactive
        // neighbours (e.g. water) re-evaluate on the next game tick.
        self.notify_block_and_neighbors(wx, wy, wz);
        true
    }

    #[inline]
    pub fn log_axis_at(&self, wx: i32, wy: i32, wz: i32) -> LogAxis {
        match self.chunk_at_world(wx, wy, wz) {
            Some((s, lx, ly, lz)) => s.log_axis(lx, ly, lz),
            None => LogAxis::Y,
        }
    }

    /// Place a single-cell log and record its axis before relighting/remeshing.
    /// Missing/vertical axes are represented sparsely, so normal trees keep no extra state.
    pub fn place_log(&mut self, pos: IVec3, block: Block, axis: LogAxis) -> bool {
        if !block.is_log() || !self.materialize_section_at(pos) {
            return false;
        }
        let Some((section_pos, _, _, _)) = Self::split_world(pos.x, pos.y, pos.z) else {
            return false;
        };
        let Some((section, lx, ly, lz)) = self.chunk_at_world_mut(pos.x, pos.y, pos.z) else {
            return false;
        };
        section.set_block(lx, ly, lz, block);
        section.set_log_axis(lx, ly, lz, axis);
        section.modified = true;
        self.refresh_particle_emitter_index(section_pos);
        if self.update_column_height_after_set(pos.x, pos.y, pos.z, true) {
            self.mark_heightmap_light_dirty_around(section_pos.chunk_pos());
        }
        self.refresh_region(&[pos]);
        true
    }

    /// Keep the column surface heightmap exact after one block change at world
    /// `(wx,wy,wz)`. Placing a solid block raises the surface; removing the current
    /// top block rescans downward through the loaded sections for the next one.
    pub(super) fn update_column_height_after_set(
        &mut self,
        wx: i32,
        wy: i32,
        wz: i32,
        solid: bool,
    ) -> bool {
        let lx = (wx & 0x0F) as usize;
        let lz = (wz & 0x0F) as usize;
        if solid {
            let cpos = ChunkPos::new(
                wx.div_euclid(SECTION_SIZE as i32),
                wz.div_euclid(SECTION_SIZE as i32),
            );
            let col = self.ensure_column(cpos);
            let old = col.surface_y(lx, lz);
            col.raise_surface(lx, lz, wy);
            let changed = wy > old;
            // wy == old replaced the visible surface block in place: the
            // heightmap is unchanged but surface consumers (map sampling)
            // still need the revision to move.
            if wy >= old {
                self.bump_column_payload_revision(cpos);
            }
            return changed;
        }
        let cur = match self.column_at(wx, wz) {
            Some(c) => c.surface_y(lx, lz),
            None => return false,
        };
        if wy != cur {
            return false; // removed a block that wasn't the surface — heightmap unchanged.
        }
        let mut new_top = NO_SURFACE;
        for y in (WORLD_MIN_Y..wy).rev() {
            if self.chunk_block(wx, y, wz) != 0 {
                new_top = y;
                break;
            }
        }
        if let Some(col) = self.column_at_mut(wx, wz) {
            col.set_surface_y(lx, lz, new_top);
        }
        let changed = new_top != cur;
        if changed {
            self.bump_column_payload_revision(ChunkPos::new(
                wx.div_euclid(SECTION_SIZE as i32),
                wz.div_euclid(SECTION_SIZE as i32),
            ));
        }
        changed
    }
}
