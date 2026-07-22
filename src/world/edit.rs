use crate::block::{Block, ShapeFamily};
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
        match block.shape_family() {
            ShapeFamily::Model => self
                .model_group(pos)
                .map(|(_, _, cells)| cells)
                .unwrap_or_else(|| vec![pos]),
            ShapeFamily::Door => self
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
        match block.shape_family() {
            ShapeFamily::Model => {
                let _ = self.remove_model_block(pos);
            }
            ShapeFamily::Door => {
                let _ = self.remove_door(pos);
            }
            _ => {
                // Same residue rule as the server's authoritative break, so a
                // predicted ice break leaves the same water the server will.
                let below = Block::from_id(self.chunk_block(pos.x, pos.y - 1, pos.z));
                let _ = self.set_block_world(pos.x, pos.y, pos.z, block.break_residue(below));
            }
        }
        Some((block, cells))
    }

    /// Set a block at world coords. Updates the column's visible surface and
    /// direct-sky cover, marks light dirty across the change's exact influence
    /// reach and queues a remesh of every section whose mesh samples the cell,
    /// so the next `tick_mesh_budget` refreshes cached light and rebuilds
    /// meshes. Returns false if the section is not loaded or `wy` is out of
    /// range. In-memory only.
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
        let old = {
            let Some(s) = self.section_mut(pos) else {
                return false;
            };
            let old = Block::from_id(s.block_raw(lx, ly, lz));
            let was_light_dirty = s.light_dirty;
            s.set_block(lx, ly, lz, b);
            s.modified = true;
            // The raw setter flagged this section's light; the invalidation
            // below re-marks exactly what the edit can influence (possibly
            // nothing at all), so hand the decision back to it. The setter's
            // revision bump stands — an in-flight bake of the pre-edit blocks
            // must still be rejected.
            if !was_light_dirty {
                s.mark_light_clean();
            }
            old
        };
        self.refresh_particle_emitter_index(pos);
        if let Some(change) = self.update_column_heights_after_set(wx, wy, wz, b) {
            self.mark_sky_cover_edited_at(wx, wz, change);
        }

        // Re-mesh exactly the sections whose pads sample this cell so border
        // face culling, AO, and smooth light stay correct across seams.
        self.queue_dirty_meshes_sampling_cell(wx, wy, wz);
        // A Layer-3 custom shape's bake depends on this cell's block + state, so
        // drop the cached bake here + at each face neighbour and re-mark any
        // custom cell dirty for the next bake pump (the same hook the replica's
        // ingest calls, so client prediction bakes the same cells).
        self.mark_custom_bake_edit(wx, wy, wz, b);
        // Plane openness may have changed; deep-visibility must re-evaluate.
        self.vis_dirty = true;

        // Announce the change: re-lights the influence reach and lets reactive
        // neighbours (e.g. water) re-evaluate on the next game tick. A proven
        // light-identical replacement (glass into air, stone variants) skips
        // the relight entirely; a plain solid⇄air edit relights only as far
        // as the light actually present at the cell can carry a change. A
        // proven walkability-identical replacement (a grazed grass tuft, a
        // crop stage, a hydration swap) likewise skips the confinement
        // invalidation feed (see `edit_nav_equivalent`).
        let nav_relevant = !super::tick::edit_nav_equivalent(old, b);
        if old.has_same_light_behavior(b) {
            self.notify_light_equivalent_change_nav(wx, wy, wz, nav_relevant);
        } else {
            let radius = self.edit_light_reach(wx, wy, wz, old, b);
            self.notify_block_change_with_light_radius_nav(wx, wy, wz, radius, nav_relevant);
        }
        true
    }

    /// Swap a placed cube block's id in place while PRESERVING everything else
    /// the cell owns — the sibling block-entity maps (machine state, container,
    /// entity facing; `set_block` never touches them) and the cell's mod KV
    /// (which `set_block` clears, so it is carried across explicitly). The
    /// cube sibling of [`World::swap_model_block`]: the same placed machine
    /// changing costume (`furnace` ⇄ `furnace_lit`). Announces itself through
    /// the ordinary block-write lanes (delta capture, relight, remesh, block
    /// updates, save `modified`) — a skin swap needs no bespoke promotion.
    pub(crate) fn swap_block_skin(&mut self, pos: IVec3, to: Block) -> bool {
        let Some((chunk, lx, ly, lz)) = self.chunk_at_world_mut(pos.x, pos.y, pos.z) else {
            return false;
        };
        let kv = chunk.cell_kv_take(lx, ly, lz);
        if !self.set_block_world(pos.x, pos.y, pos.z, to) {
            // The write was refused (stream-finality guard): put the KV back
            // and leave the cell exactly as it was.
            if let (Some(kv), Some((chunk, lx, ly, lz))) =
                (kv, self.chunk_at_world_mut(pos.x, pos.y, pos.z))
            {
                chunk.cell_kv_restore(lx, ly, lz, kv);
            }
            return false;
        }
        if let Some(kv) = kv {
            if let Some((chunk, lx, ly, lz)) = self.chunk_at_world_mut(pos.x, pos.y, pos.z) {
                chunk.cell_kv_restore(lx, ly, lz, kv);
            }
        }
        true
    }

    /// How far (in cells, L1) the light change from replacing `old` with `new`
    /// at one cell can possibly propagate — `-1` when it provably cannot
    /// change any light value at all. Only the plain full-cube transitions
    /// are bounded; anything stateful or emitting falls back to the full
    /// flood reach. Sound because a value `v` at the cell decays 2 per step:
    /// no cell past `v/2 - 1` can observe a difference. The cell's own light
    /// cubes still hold their pre-edit values when this runs.
    fn edit_light_reach(&self, wx: i32, wy: i32, wz: i32, old: Block, new: Block) -> i32 {
        if old.light_emission() != 0 || new.light_emission() != 0 {
            return Self::LIGHT_REACH;
        }
        let value_at = |x: i32, y: i32, z: i32| {
            self.skylight_at_world(x, y, z)
                .max(self.blocklight_at_world(x, y, z)) as i32
        };
        let v = if old.is_opaque() && new == Block::Air {
            // Opening a cell: whatever enters comes through the six faces.
            crate::mathh::FACE_NEIGHBORS
                .into_iter()
                .map(|d| value_at(wx + d.x, wy + d.y, wz + d.z))
                .max()
                .unwrap_or(0)
        } else if old == Block::Air && new.is_opaque() {
            // Closing a cell: only paths that ran through its own value die.
            value_at(wx, wy, wz)
        } else {
            return Self::LIGHT_REACH;
        };
        if v == 0 {
            -1
        } else {
            (v / 2 - 1).clamp(0, Self::LIGHT_REACH)
        }
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
