use super::Game;
use crate::world::TerrainRenderHandoff;

impl Game {
    /// The renderer's mesh handoff — the REPLICA's meshes (the server world
    /// never meshes).
    #[inline]
    pub(crate) fn terrain_render_handoff(&mut self) -> TerrainRenderHandoff<'_> {
        self.replica.terrain_render_handoff()
    }
}
