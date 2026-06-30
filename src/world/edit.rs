use crate::block::Block;
use crate::chunk::{ChunkPos, SECTION_SIZE, WORLD_MIN_Y};
use crate::column::NO_SURFACE;
use crate::section::SectionSummary;
use std::sync::Arc;

use super::store::World;

impl World {
    /// Set a block at world coords. Updates the column surface heightmap, marks the
    /// owning section's light plus its full 3×3×3 neighbourhood dirty so the next
    /// `tick_mesh_budget` refreshes cached light and rebuilds meshes. Returns false
    /// if the section is not loaded or `wy` is out of range. In-memory only.
    pub fn set_block_world(&mut self, wx: i32, wy: i32, wz: i32, b: Block) -> bool {
        let Some((pos, lx, ly, lz)) = Self::split_world(wx, wy, wz) else {
            return false;
        };
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
            let Some(s) = self.sections.get_mut(&pos).map(Arc::make_mut) else {
                return false;
            };
            s.set_block(lx, ly, lz, b);
            s.modified = true;
        }
        if self.update_column_height_after_set(wx, wy, wz, b != Block::Air) {
            self.mark_heightmap_light_dirty_around(pos.chunk_pos());
        }

        // Re-mesh the 3×3×3 so the border flood, vertex light sampling, and
        // cross-section face culling remain correct.
        self.mark_dirty_neighborhood(pos, true);

        // Announce the change: re-lights the neighbourhood and lets reactive
        // neighbours (e.g. water) re-evaluate on the next game tick.
        self.notify_block_and_neighbors(wx, wy, wz);
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
            return wy > old;
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
        new_top != cur
    }
}
