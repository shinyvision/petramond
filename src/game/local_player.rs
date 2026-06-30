use crate::mathh::Vec3;
use crate::player::{self, Input, Player};

use super::{Game, GameInput};

impl Game {
    pub(super) fn apply_camera_input(&mut self, input: &GameInput) {
        if !input.gameplay_enabled {
            return;
        }
        let (dx, dy) = input.look_delta;
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        const SENS: f32 = 0.0025;
        self.player.rotate(-dx * SENS, -dy * SENS);
        // Mirror the player's look onto the camera now (before this tick's
        // movement + raycast read `cam.forward()`), the same way `tick_player`
        // mirrors `player.eye()` onto `cam.pos`.
        self.cam.yaw = self.player.yaw;
        self.cam.pitch = self.player.pitch;
    }

    pub(super) fn apply_hotbar_input(&mut self, input: &GameInput) {
        if input.gameplay_enabled && input.hotbar_scroll != 0 {
            self.player.inventory.scroll_active(input.hotbar_scroll);
        }
    }

    pub(super) fn tick_player(&mut self, dt: f32, input: &GameInput) {
        let spectator = self.player.is_spectator();
        let f = self.cam.forward();
        let fwd = if spectator {
            f
        } else {
            Vec3::new(f.x, 0.0, f.z).normalize_or_zero()
        };
        let right = self.cam.right();
        let mut wishdir = Vec3::ZERO;

        if input.gameplay_enabled {
            if input.movement.forward {
                wishdir += fwd;
            }
            if input.movement.backward {
                wishdir -= fwd;
            }
            if input.movement.right {
                wishdir += right;
            }
            if input.movement.left {
                wishdir -= right;
            }
            if spectator {
                if input.movement.jump {
                    wishdir += Vec3::Y;
                }
                if input.movement.sneak {
                    wishdir -= Vec3::Y;
                }
            }
        }

        let player_input = Input {
            wishdir: wishdir.normalize_or_zero(),
            jump: input.gameplay_enabled && input.movement.jump,
            sprint: input.gameplay_enabled && input.movement.sprint,
        };

        if spectator || self.player.columns_loaded(&self.world) {
            let mut remaining = dt.min(0.25);
            while remaining > 0.0 {
                let step = remaining.min(player::DT_MAX);
                self.player.update(step, &self.world, player_input);
                remaining -= step;
            }
        }

        self.cam.pos = self.player.eye();
    }

    /// Push the player out of any mob it overlaps, per frame. The mobs sit at their
    /// last-tick positions (fixed between ticks), so as the player moves each frame the
    /// overlap - and the push - track the player smoothly; applied as a small
    /// collision-resolved displacement (the push *velocity* over this frame's `dt`), it
    /// never accumulates or fights the movement controller. A noclip spectator has no body
    /// to jostle. The mobs' own half of the push runs on the tick (`game_tick_step`).
    pub(super) fn apply_mob_push(&mut self, dt: f32) {
        if self.player.is_spectator() {
            return;
        }
        let body = crate::mob::Body::new(self.player.pos, player::HALF_W, player::HEIGHT);
        let push = self.world.mobs().push_on_player(body);
        if push != Vec3::ZERO {
            self.player.shove(push * dt, &self.world);
        }
        self.cam.pos = self.player.eye();
    }

    pub(super) fn tick_world(&mut self) {
        let cam_cx = (self.cam.pos.x as i32) >> 4;
        let cam_cy = (self.cam.pos.y.floor() as i32).div_euclid(16);
        let cam_cz = (self.cam.pos.z as i32) >> 4;
        let forward = self.cam.forward();
        self.world
            .update_load_facing(cam_cx, cam_cy, cam_cz, forward.x, forward.z);
        let _ = self.world.poll();
    }

    pub(super) fn refresh_target(&mut self) {
        let block_hit = Player::raycast_with_dist(self.cam.pos, self.cam.forward(), &self.world);
        self.look = block_hit.map(|(h, _)| h);
        let block_dist = block_hit.map(|(_, d)| d).unwrap_or(player::REACH);
        self.targeted_mob = self.closest_mob(self.cam.pos, self.cam.forward(), block_dist);
        if self.targeted_mob.is_some() {
            self.look = None;
        }
    }

    /// The closest mob in front of the eye whose AABB the ray enters within `max_dist`
    /// (and within reach), skipping dead corpses. `max_dist` is the block hit distance,
    /// so a mob *behind* the block isn't targeted (the block occludes it).
    pub(super) fn closest_mob(&self, eye: Vec3, dir: Vec3, max_dist: f32) -> Option<usize> {
        let limit = max_dist.min(player::REACH);
        let mut best: Option<(usize, f32)> = None;
        for (i, m) in self.world.mobs().instances().iter().enumerate() {
            if m.is_dead() {
                continue; // a corpse can't be targeted
            }
            let (min, max) = m.aabb();
            if let Some(t) = player::ray_vs_aabb(eye, dir, min, max) {
                if t <= limit && best.is_none_or(|(_, bt)| t < bt) {
                    best = Some((i, t));
                }
            }
        }
        best.map(|(i, _)| i)
    }
}
