use super::Game;
use crate::world::TerrainRenderHandoff;

impl Game {
    #[inline]
    pub(crate) fn terrain_render_handoff(&mut self) -> TerrainRenderHandoff<'_> {
        self.world.terrain_render_handoff()
    }
}
