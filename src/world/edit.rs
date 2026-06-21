use crate::block::Block;
use crate::chunk::{ChunkPos, CHUNK_SY};

use super::store::World;

impl World {
    /// Set a block at world coords. Marks the owning chunk's light plus its full
    /// 3x3 neighbourhood dirty so the next `tick_mesh_budget` refreshes any
    /// cached bands whose border flood may have changed, then rebuilds their
    /// meshes. Returns false if the chunk is not loaded or `wy` is out of range.
    /// In-memory only.
    pub fn set_block_world(&mut self, wx: i32, wy: i32, wz: i32, b: Block) -> bool {
        if wy < 0 || wy >= CHUNK_SY as i32 {
            return false;
        }
        let cx = wx >> 4;
        let cz = wz >> 4;
        let lx = (wx & 0x0F) as usize;
        let lz = (wz & 0x0F) as usize;
        let pos = ChunkPos::new(cx, cz);
        {
            let Some(c) = self.chunks.get_mut(&pos) else {
                return false;
            };
            c.set_block(lx, wy as usize, lz, b);
        }
        self.invalidate_section_visibility(pos);

        // Re-light and re-mesh the 3x3 so border flood, vertex light sampling,
        // and cross-chunk face culling remain correct.
        self.mark_light_dirty_neighborhood(pos, true);
        self.mark_dirty_neighborhood(pos, true);

        // Announce the change so reactive neighbours (e.g. water) re-evaluate on
        // the next game tick.
        self.notify_block_and_neighbors(wx, wy, wz);
        true
    }
}
