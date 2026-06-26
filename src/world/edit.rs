use crate::block::Block;

use super::store::World;

impl World {
    /// Set a block at world coords. Marks the owning chunk's light plus its full
    /// 3x3 neighbourhood dirty so the next `tick_mesh_budget` refreshes any
    /// cached bands whose border flood may have changed, then rebuilds their
    /// meshes. Returns false if the chunk is not loaded or `wy` is out of range.
    /// In-memory only.
    pub fn set_block_world(&mut self, wx: i32, wy: i32, wz: i32, b: Block) -> bool {
        let Some((pos, lx, ly, lz)) = Self::split_world(wx, wy, wz) else {
            return false;
        };
        {
            let Some(c) = self.chunks.get_mut(&pos) else {
                return false;
            };
            c.set_block(lx, ly, lz, b);
            c.modified = true;
        }
        self.invalidate_section_visibility(pos);

        // Re-mesh the 3x3 so the border flood, vertex light sampling, and
        // cross-chunk face culling remain correct.
        self.mark_dirty_neighborhood(pos, true);

        // Announce the change: this re-lights the 3x3 (border flood) and lets
        // reactive neighbours (e.g. water) re-evaluate on the next game tick — the
        // relight rides along with the announce (see `notify_block_and_neighbors`).
        self.notify_block_and_neighbors(wx, wy, wz);
        true
    }
}
