use crate::block::{Block, RenderShape};
use crate::block_state::LogAxis;
use crate::chunk::{ChunkPos, SECTION_SIZE, WORLD_MIN_Y};
use crate::column::NO_SURFACE;
use crate::mathh::IVec3;
use crate::section::SectionSummary;

use super::store::{SkyCoverChange, World};

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

    /// Set a block at world coords. Updates the column's visible surface and
    /// direct-sky cover, marks the owning section's light plus its full 3×3×3
    /// neighbourhood dirty so the next `tick_mesh_budget` refreshes cached light
    /// and rebuilds meshes. Returns false if the section is not loaded or `wy` is
    /// out of range. In-memory only.
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
        if let Some(change) = self.update_column_heights_after_set(wx, wy, wz, b) {
            self.mark_sky_cover_edited_around(pos.chunk_pos(), change);
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
        self.refresh_region(&[pos]);
        true
    }

    /// Keep the visible surface and direct-skylight cover exact after one block
    /// change. Clear blocks may raise the visible surface without moving sky
    /// cover; removing either current top rescans downward for its next match.
    /// Returns the vertical cover-change envelope used to invalidate loaded
    /// sections whose bake can observe the move.
    pub(super) fn update_column_heights_after_set(
        &mut self,
        wx: i32,
        wy: i32,
        wz: i32,
        block: Block,
    ) -> Option<SkyCoverChange> {
        let lx = (wx & 0x0F) as usize;
        let lz = (wz & 0x0F) as usize;
        let cpos = ChunkPos::new(
            wx.div_euclid(SECTION_SIZE as i32),
            wz.div_euclid(SECTION_SIZE as i32),
        );
        let (old_surface, old_sky_cover) = match self.column_at(wx, wz) {
            Some(c) => (c.surface_y(lx, lz), c.sky_cover_y(lx, lz)),
            None => return None,
        };

        let mut new_surface = old_surface;
        let mut surface_payload_changed = false;
        if block != Block::Air {
            if wy > old_surface {
                new_surface = wy;
                surface_payload_changed = true;
            } else if wy == old_surface {
                // Same-height replacement can change the map's visible material.
                surface_payload_changed = true;
            }
        } else if wy == old_surface {
            new_surface = NO_SURFACE;
            for y in (WORLD_MIN_Y..wy).rev() {
                if self.chunk_block(wx, y, wz) != Block::Air.id() {
                    new_surface = y;
                    break;
                }
            }
            surface_payload_changed = new_surface != old_surface;
        }

        let mut new_sky_cover = old_sky_cover;
        if !block.transmits_direct_skylight() {
            if wy > old_sky_cover {
                new_sky_cover = wy;
            }
        } else if wy == old_sky_cover {
            new_sky_cover = NO_SURFACE;
            for y in (WORLD_MIN_Y..wy).rev() {
                let below = Block::from_id(self.chunk_block(wx, y, wz));
                if !below.transmits_direct_skylight() {
                    new_sky_cover = y;
                    break;
                }
            }
        }

        let sky_cover_change = SkyCoverChange::between(old_sky_cover, new_sky_cover);
        if new_surface != old_surface || sky_cover_change.is_some() {
            let col = self.columns.get_mut(&cpos).expect("column was read above");
            col.set_surface_y(lx, lz, new_surface);
            col.set_sky_cover_y(lx, lz, new_sky_cover);
        }
        if surface_payload_changed || sky_cover_change.is_some() {
            self.bump_column_payload_revision(cpos);
        }
        sky_cover_change
    }
}
