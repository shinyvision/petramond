use super::Game;

impl Game {
    pub(super) fn sync_sleep_camera_on_open(
        &mut self,
        self_events: &petramond::net::protocol::SelfEvents,
    ) {
        if self_events.open_screen != Some(petramond::net::protocol::OpenScreen::Sleep) {
            return;
        }
        self.local.cam.yaw = self.local.player.yaw;
        self.local.cam.pitch = self.local.player.pitch;
        self.sync_camera_to_player_eye(0.0);
    }

    #[inline]
    pub(super) fn tick_alpha(&self) -> f32 {
        self.replica.entities.alpha()
    }

    pub(super) fn held_item_light(&self) -> (u8, petramond_world::light::BlockLight6) {
        let c = self.local.cam.pos.block();
        self.replica
            .world
            .data()
            .dynamic_light_at_world(c.x, c.y, c.z)
    }

    pub(super) fn tick_mesh_budget(&mut self) {
        const MESH_BUDGET: usize = 256;
        self.replica.world.tick_mesh_budget(MESH_BUDGET);
    }
}
