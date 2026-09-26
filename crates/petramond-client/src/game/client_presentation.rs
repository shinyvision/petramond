//! Game-owned client presentation/activity helpers.
//!
//! These methods expose the interpolation clock, held-item light and the
//! replica's mesh budget to sibling game modules. Only REPLICATED state is
//! read here (the stores + the replica world — the sim lives on the server
//! thread); the world effects themselves (particles, dig feedback, block
//! swings) live in [`WorldFx`](super::world_fx::WorldFx).

use super::Game;

impl Game {
    /// Falling asleep tucks the local player in on the tick (`ServerGame`'s
    /// bed stage); the camera mirror is presentation, applied off the
    /// replicated sleep-open one-shot right after the fixed ticks — before
    /// any presentation read. The tucked transform was already adopted into
    /// the predicted player (`adopt_authoritative_transform` runs first).
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

    /// Fraction (`0..1`) into the next fixed tick, the blend factor the scene uses to
    /// interpolate each entity's render pose between its previous and current tick, so the
    /// mobs and dropped items (which simulate at 20 TPS) move smoothly at any frame rate.
    /// Measured client-side from the arrival time of the last applied
    /// `TickUpdate` ([`tick::ReplicaClock`]) — the server accumulator lives on
    /// its own thread now.
    #[inline]
    pub(super) fn tick_alpha(&self) -> f32 {
        self.replica.entities.alpha()
    }

    /// Two-channel light at the player's eye, for lighting the first-person hand
    /// / held item: it brightens AND takes the colour of nearby block light,
    /// and the torch channel keeps it lit at night.
    pub(super) fn held_item_light(&self) -> (u8, petramond_world::light::BlockLight6) {
        let c = self.local.cam.pos.block();
        self.replica.world.data().dynamic_light_at_world(c.x, c.y, c.z)
    }

    pub(super) fn tick_mesh_budget(&mut self) {
        // Generous count — the pump's own time budget (MESH_SUBMIT_TIME_BUDGET) is
        // what actually protects the frame; a small count here just frame-quantized
        // streaming bursts into a multi-second trickle. Pumps the REPLICA's
        // mesh + light queues (the server world never meshes).
        // High enough that the real per-frame limits are the mesh pump's
        // in-flight window and its submit-time budget, not this count: 64
        // admission-limited RD32 flight meshing while the workers sat idle.
        const MESH_BUDGET: usize = 256;
        self.replica.world.tick_mesh_budget(MESH_BUDGET);
        let replica = &self.replica.world;
        self.net
            .report_terrain_backlog(|| replica.terrain_presentation_backlog());
    }
}
