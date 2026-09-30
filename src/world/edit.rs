use crate::world::WorldData;
use crate::world::{World, WorldSide};
use petramond_math::math::IVec3;
use petramond_world::block::Block;
use petramond_world::block_state::LogAxis;
use petramond_world::chunk::{ChunkPos, SECTION_SIZE, WORLD_MIN_Y};
use petramond_world::column::NO_SURFACE;
use petramond_world::section::SectionSummary;

use super::cell_change::{CellChange, ChangeKind};
use super::store::SkyCoverChange;

impl<S: WorldSide> World<S> {
    pub fn break_footprint_cells(&self, pos: IVec3) -> Vec<IVec3> {
        self.compound_cells(pos).unwrap_or_else(|| vec![pos])
    }

    pub fn compound_cells(&self, pos: IVec3) -> Option<Vec<IVec3>> {
        let block = Block::from_id(self.data.chunk_block(pos.x, pos.y, pos.z));
        let state = petramond_world::block::ShapeNeighborhood::shape_state(&self.data, pos);
        let members = block.compound_members(pos, state)?;
        Some(members.into_iter().map(|(cell, _)| cell).collect())
    }

    pub fn remove_compound(&mut self, pos: IVec3) -> Option<Vec<IVec3>> {
        let cells = self.compound_cells(pos)?;
        let mut changes = Vec::with_capacity(cells.len());
        for &c in &cells {
            if let Some((chunk, lx, ly, lz)) = self.data.chunk_at_world_mut(c.x, c.y, c.z) {
                let old = chunk.block(lx, ly, lz);
                chunk.set_block(lx, ly, lz, Block::Air);
                chunk.modified = true;
                changes.push(CellChange::new(c, old, ChangeKind::Place));
            }
        }
        self.apply_cell_changes(&changes);
        Some(cells)
    }

    pub fn clear_broken_block(&mut self, pos: IVec3) -> Option<(Block, Vec<(IVec3, u16)>)> {
        let block = Block::from_id(self.data.chunk_block(pos.x, pos.y, pos.z));
        if block.hardness() < 0.0 {
            return None;
        }
        let cells: Vec<(IVec3, u16)> = self
            .break_footprint_cells(pos)
            .into_iter()
            .map(|c| (c, self.data.chunk_block(c.x, c.y, c.z)))
            .collect();
        if self.remove_compound(pos).is_none() {
            let below = Block::from_id(self.data.chunk_block(pos.x, pos.y - 1, pos.z));
            let _ = self.set_block_world(pos.x, pos.y, pos.z, block.break_residue(below));
        }
        Some((block, cells))
    }

    pub fn set_block_world(&mut self, wx: i32, wy: i32, wz: i32, b: Block) -> bool {
        let Some((pos, lx, ly, lz)) = WorldData::split_world(wx, wy, wz) else {
            return false;
        };
        if !self.data.stream_writable(pos) {
            return false;
        }
        if !self.data.sections.contains_key(&pos) {
            let summary = self.data.section_summary(pos);
            let absent_air = matches!(summary, SectionSummary::Empty | SectionSummary::Unknown);
            if (b == Block::Air && absent_air) || !self.materialize_section(pos) {
                return false;
            }
        }
        let old = {
            let Some(s) = self.data.section_mut(pos) else {
                return false;
            };
            let old = Block::from_id(s.block_raw(lx, ly, lz));
            let was_light_dirty = s.light_dirty;
            s.set_block(lx, ly, lz, b);
            s.modified = true;
            if !was_light_dirty {
                s.mark_light_clean();
            }
            old
        };
        self.apply_cell_changes(&[CellChange::new(
            IVec3::new(wx, wy, wz),
            old,
            ChangeKind::Write,
        )]);
        true
    }

    pub fn swap_block_skin(&mut self, pos: IVec3, to: Block) -> bool {
        let Some((sp, ..)) = WorldData::split_world(pos.x, pos.y, pos.z) else {
            return false;
        };
        if !self.data.stream_writable(sp) {
            return false;
        }
        let Some((chunk, lx, ly, lz)) = self.data.chunk_at_world_mut(pos.x, pos.y, pos.z) else {
            return false;
        };
        let old = chunk.block(lx, ly, lz);
        let kv = chunk.cell_kv_take(lx, ly, lz);
        let state = chunk.cell_state(lx, ly, lz);
        chunk.set_block(lx, ly, lz, to);
        if let Some(kv) = kv {
            chunk.cell_kv_restore(lx, ly, lz, kv);
        }
        if !state.is_empty() {
            chunk.set_cell_state(lx, ly, lz, state);
        }
        chunk.modified = true;
        self.apply_cell_changes(&[CellChange::new(pos, old, ChangeKind::Costume)]);
        true
    }

    /// How far (in cells, L1) the light change from replacing `old` with `new`
    /// at one cell can possibly propagate — `-1` when it provably cannot
    /// change any light value at all. Only the plain full-cube transitions
    /// are bounded; anything stateful or emitting falls back to the full
    /// flood reach. Sound because a value `v` at the cell decays 2 per step:
    /// no cell past `v/2 - 1` can observe a difference. The cell's own light
    /// cubes still hold their pre-edit values when this runs.
    pub(super) fn edit_light_reach(
        &self,
        wx: i32,
        wy: i32,
        wz: i32,
        old: Block,
        new: Block,
    ) -> i32 {
        if old.light_emission() != 0 || new.light_emission() != 0 {
            return Self::LIGHT_REACH;
        }
        let value_at = |x: i32, y: i32, z: i32| {
            self.data
                .skylight_at_world(x, y, z)
                .max(self.data.blocklight_at_world(x, y, z)) as i32
        };
        let v = if old.is_opaque() && new == Block::Air {
            petramond_math::math::FACE_NEIGHBORS
                .into_iter()
                .map(|d| value_at(wx + d.x, wy + d.y, wz + d.z))
                .max()
                .unwrap_or(0)
        } else if old == Block::Air && new.is_opaque() {
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
        match self.data.chunk_at_world(wx, wy, wz) {
            Some((s, lx, ly, lz)) => s.log_axis(lx, ly, lz),
            None => LogAxis::Y,
        }
    }

    pub fn place_log(&mut self, pos: IVec3, block: Block, axis: LogAxis) -> bool {
        if !block.is_axial() || !self.materialize_section_at(pos) {
            return false;
        }
        let Some((section, lx, ly, lz)) = self.data.chunk_at_world_mut(pos.x, pos.y, pos.z) else {
            return false;
        };
        let old = section.block(lx, ly, lz);
        section.set_block(lx, ly, lz, block);
        section.set_log_axis(lx, ly, lz, axis);
        section.modified = true;
        self.apply_cell_changes(&[CellChange::new(pos, old, ChangeKind::Place)]);
        true
    }

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
        let column = self.data.column_at(wx, wz)?;
        let (old_surface, old_sky_cover) = (column.surface_y(lx, lz), column.sky_cover_y(lx, lz));

        let mut new_surface = old_surface;
        let mut surface_payload_changed = false;
        if block != Block::Air {
            if wy > old_surface {
                new_surface = wy;
                surface_payload_changed = true;
            } else if wy == old_surface {
                surface_payload_changed = true;
            }
        } else if wy == old_surface {
            new_surface = NO_SURFACE;
            for y in (WORLD_MIN_Y..wy).rev() {
                if self.data.chunk_block(wx, y, wz) != Block::Air.id() {
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
                let below = Block::from_id(self.data.chunk_block(wx, y, wz));
                if !below.transmits_direct_skylight() {
                    new_sky_cover = y;
                    break;
                }
            }
        }

        let sky_cover_change = SkyCoverChange::between(old_sky_cover, new_sky_cover);
        if new_surface != old_surface || sky_cover_change.is_some() {
            let col = std::sync::Arc::make_mut(
                self.data
                    .columns
                    .get_mut(&cpos)
                    .expect("column was read above"),
            );
            col.set_surface_y(lx, lz, new_surface);
            col.set_sky_cover_y(lx, lz, new_sky_cover);
        }
        if surface_payload_changed || sky_cover_change.is_some() {
            self.data.bump_column_payload_revision(cpos);
        }
        sky_cover_change
    }
}
