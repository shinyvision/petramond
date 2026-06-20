use crate::block::Block;
use crate::chunk::{ChunkPos, CHUNK_SY};

use super::store::World;

impl World {
    /// Set a block at world coords. Re-bakes the owning chunk's skylight and marks
    /// it plus its full 3x3 neighbourhood dirty so the next `tick_mesh_budget`
    /// rebuilds them. Returns false if the chunk is not loaded or `wy` is out of
    /// range. In-memory only.
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

            // This chunk's blocks changed, so its cached self-contained skylight
            // must be refreshed before any mesh samples it.
            let (band, ylo, yhi) = crate::mesh::compute_chunk_skylight(c);
            c.set_skylight(band, ylo, yhi);
        }
        self.invalidate_section_visibility(pos);

        // Re-mesh the 3x3 so border faces re-sample this chunk's changed edge
        // light and cross-chunk face culling remains correct.
        self.mark_dirty_neighborhood(pos, true);
        true
    }
}
