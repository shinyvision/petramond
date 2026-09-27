use super::Game;
use petramond::world::TerrainRenderHandoff;

impl Game {
    #[inline]
    pub fn terrain_render_handoff(&mut self) -> TerrainRenderHandoff<'_> {
        self.replica.world.terrain_render_handoff()
    }
}
