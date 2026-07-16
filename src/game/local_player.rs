//! The client's locally-simulated player: per-frame look/movement physics on
//! `Game::player`, the camera mirror, and the per-frame target refresh
//! (`Game::look`/`Game::targeted_mob`). None of this touches the sessions —
//! the results reach the sim as the next `PlayerUpdate` message.

use crate::mathh::Vec3;
use crate::player::{self, Input, Player};

use super::{Game, GameInput};

const STEP_CAMERA_SETTLE_SPEED: f32 = 12.0;
const STEP_CAMERA_EPS: f32 = 0.001;
/// Camera height above the feet while asleep: head-on-the-pillow, a touch
/// above the mattress the body is standing on (vs the standing `player::EYE`).
const SLEEP_EYE_HEIGHT: f32 = 0.25;
/// How far the first-person eye drops while sneaking (blocks) — the in-view
/// feedback that sneak is active. CAMERA ONLY: the sim eye (`player::EYE`),
/// reach, and the collision box stay full height, exactly like the sleep
/// pillow-height camera. Well inside the server's `REACH + 1` target-latch
/// slack, so a raycast from the lowered eye never trips reach validation.
const SNEAK_EYE_DROP: f32 = 0.3;
/// Exponential settle rate of the sneak eye drop — matches the body pose's
/// sneak blend rate so the first-person dip and the third-person crouch land
/// together.
const SNEAK_EYE_SETTLE_SPEED: f32 = 10.0;

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
        // Mirror the player's look onto the camera now, before this tick's
        // movement and raycast read `cam.forward()`.
        self.cam.yaw = self.player.yaw;
        self.cam.pitch = self.player.pitch;
    }

    pub(super) fn apply_hotbar_input(&mut self, input: &GameInput) {
        if input.gameplay_enabled && input.hotbar_scroll != 0 {
            // Client-owned selection (only the INDEX matters — contents are
            // session-owned); the slot rides `PlayerUpdate.hotbar_slot`. Any
            // hotbar change resets the R-key rotation cycle so the raw wire
            // counter unambiguously means "R pressed on the current selection".
            let before = self.player.inventory.active_slot();
            self.player.inventory.scroll_active(input.hotbar_scroll);
            let after = self.player.inventory.active_slot();
            if after != before {
                self.held_rotation.clear();
            }
            // Mirror into the replicated view (selection is client-owned;
            // the server never echoes it back).
            self.self_view.inventory.set_active(after);
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
            sneak: input.gameplay_enabled && input.movement.sneak,
        };
        // Stash for `build_player_update`: the wire intent must be the exact
        // input the local physics consumed this frame.
        self.predicted_input = player_input;

        // Mounted: no local physics — the body slaves to the interpolated
        // mount at the seat offset, the same glue observers apply to mounted
        // remotes, so rider and mount can never visibly separate. The intent
        // stashed above still rides `PlayerUpdate` (that IS the steering
        // input the driving mod reads server-side).
        if let Some(seat_pos) = self.self_mount.and_then(|m| self.mount_seat_pos(m)) {
            self.player.pos = seat_pos;
            self.player.vel = Vec3::ZERO;
            self.player.on_ground = true;
            self.sync_camera_to_player_eye(dt);
            return;
        }

        // Physics gates on the REPLICA's loaded columns: until the spawn area's
        // payloads land, the player holds still (exactly the fresh-world
        // stream-in wait; absent-Mixed sections would read as air and lie).
        if spectator || self.player.columns_loaded(&self.replica) {
            // Solid entities (a boat's hull) block the predicted body exactly
            // like the server's integration does — sourced from the
            // interpolated replicated rows, the same transform they render at.
            let obstacles = self.solid_entity_obstacles();
            let mut remaining = dt.min(0.25);
            while remaining > 0.0 {
                let step = remaining.min(player::DT_MAX);
                self.player
                    .update_with_obstacles(step, &self.replica, player_input, &obstacles);
                remaining -= step;
            }
        }

        self.sync_camera_to_player_eye(dt);
    }

    /// Dynamic collision boxes for the local player's physics this frame:
    /// every live SOLID entity (see `MobCollision::Solid`) at its
    /// interpolated replicated transform — except the own mount, whose box
    /// the slaved rider sits inside.
    pub(super) fn solid_entity_obstacles(&self) -> Vec<crate::collision::DynBox> {
        let alpha = self.tick_alpha();
        let own_mount = self.self_mount.map(|(id, _)| id);
        let mut out = Vec::new();
        for entry in self.replicated_mobs.iter() {
            let row = &entry.curr;
            if row.dead || Some(row.id) == own_mount {
                continue;
            }
            let d = crate::mob::def(crate::mob::Mob(row.kind_id));
            if d.collision != crate::mob::MobCollision::Solid {
                continue;
            }
            let (pos, yaw) = entry.interpolated_pose(alpha);
            crate::mob::solid_boxes(row.id, pos, yaw, d.size, &mut out);
        }
        out
    }

    /// The world-space seat position of `(mob id, seat)` on the INTERPOLATED
    /// replicated mount this frame, or `None` when the mob isn't replicated
    /// (yet) or the seat isn't declared — the caller keeps its current
    /// transform and waits for the rows to agree.
    fn mount_seat_pos(&self, (mob_id, seat): (u64, u8)) -> Option<Vec3> {
        self.replicated_mobs
            .interpolated_seat_pose(mob_id, seat, self.tick_alpha())
            .map(|(pos, _)| pos)
    }

    /// The BODY yaw a seated local player renders with: the interpolated
    /// mount's facing in player-yaw space (mob yaw 0 faces `-Z`, player body
    /// yaw 0 faces `+Z` — π apart). A rider sits square in its seat; only the
    /// head follows the look (see `collect_player`).
    pub(super) fn mount_body_yaw(&self) -> Option<f32> {
        let (mob_id, seat) = self.self_mount?;
        let (_, yaw) =
            self.replicated_mobs
                .interpolated_seat_pose(mob_id, seat, self.tick_alpha())?;
        Some(crate::game::body_pose::wrap_angle(
            yaw + std::f32::consts::PI,
        ))
    }

    /// Push the player out of any soft entity body it overlaps — mobs and remote
    /// players — per frame. The bodies sit at their last-batch positions (fixed between
    /// ticks), so as the player moves each frame the overlap - and the push - track the
    /// player smoothly; applied as a small collision-resolved displacement (the push
    /// *velocity* over this frame's `dt`), it never accumulates or fights the movement
    /// controller. A noclip spectator has no body to jostle. The mobs' own half of the
    /// push runs on the tick (`game_tick_step`); a remote PLAYER's half runs on that
    /// player's own client through this same rule against ITS replicated rows — each
    /// client only ever shoves itself, and the shove reaches the server in the next
    /// `PlayerUpdate`, so player↔player separation is symmetric without any server-side
    /// push step.
    pub(super) fn apply_entity_push(&mut self, dt: f32) {
        // A mounted body is slaved to its seat: nothing may jostle it (its
        // own mount overlaps it every frame).
        if self.player.is_spectator() || self.self_mount.is_some() {
            return;
        }
        let body = self.player.body();
        let mut push = Vec3::ZERO;
        for entry in self.replicated_mobs.iter() {
            if entry.curr.dead {
                continue; // a ragdolling corpse doesn't push
            }
            let d = crate::mob::def(crate::mob::Mob(entry.curr.kind_id));
            if d.collision == crate::mob::MobCollision::Solid {
                // Rigid geometry in the player's own resolver — a soft push
                // on top would fight the contact (skating a deck-stander).
                continue;
            }
            let mob = crate::body::Body::new(entry.curr.pos, d.size.half_width, d.size.height);
            if let Some(p) = crate::body::separation(body, mob) {
                push += p;
            }
        }
        for remote in self.remote_players.iter() {
            let Some(other) = remote.push_body() else {
                continue; // hidden (spectator/dead) or asleep in a bed
            };
            if let Some(p) = crate::body::separation(body, other) {
                push += p;
            }
        }
        if push != Vec3::ZERO {
            self.player.shove(push * dt, &self.replica);
            self.sync_camera_to_player_eye(dt);
        }
    }

    pub(super) fn sync_camera_to_player_eye(&mut self, dt: f32) {
        let target = self.player.eye();
        let eye_dy = target.y - self.last_player_eye_y;
        let grounded_still = self.player.on_ground && self.player.vel.y.abs() <= STEP_CAMERA_EPS;
        if self.player.is_spectator() {
            self.camera_step_y_offset = 0.0;
        } else if grounded_still
            && eye_dy > STEP_CAMERA_EPS
            && eye_dy <= crate::collision::STEP_HEIGHT + STEP_CAMERA_EPS
        {
            let max_lag = crate::collision::STEP_HEIGHT * 1.5;
            self.camera_step_y_offset = (self.camera_step_y_offset - eye_dy).max(-max_lag);
        } else if grounded_still
            && self.predicted_input.sneak
            && eye_dy < -STEP_CAMERA_EPS
            && eye_dy >= -(crate::collision::STEP_HEIGHT + STEP_CAMERA_EPS)
        {
            // The sneak snap-down: physics dropped the feet onto the lower step
            // instantly (grounded throughout, so the guard never let go); the
            // camera starts the step ABOVE and settles down — the mirror of the
            // step-up glide. Sneak-gated so ordinary landing dips keep their
            // un-eased feel.
            let max_lag = crate::collision::STEP_HEIGHT * 1.5;
            self.camera_step_y_offset = (self.camera_step_y_offset - eye_dy).min(max_lag);
        }

        let settle = 1.0 - (-STEP_CAMERA_SETTLE_SPEED * dt.max(0.0)).exp();
        self.camera_step_y_offset += (0.0 - self.camera_step_y_offset) * settle;
        if self.camera_step_y_offset.abs() <= STEP_CAMERA_EPS {
            self.camera_step_y_offset = 0.0;
        }

        // The sneak eye drop eases toward its target so crouching dips instead
        // of teleporting; the same intent drives the third-person stance blend.
        let sneak_target = if !self.player.is_spectator() && self.predicted_input.sneak {
            -SNEAK_EYE_DROP
        } else {
            0.0
        };
        let sneak_settle = 1.0 - (-SNEAK_EYE_SETTLE_SPEED * dt.max(0.0)).exp();
        self.camera_sneak_y_offset += (sneak_target - self.camera_sneak_y_offset) * sneak_settle;

        // Lying in bed: the body stays a standing collision box on the
        // mattress (physics unchanged), but the camera drops to pillow height
        // so the player visibly lies down rather than standing on the bed.
        // Sleep state reads the replicated self view.
        let eye_y = if self.self_view.sleeping.is_some() {
            self.player.pos.y + SLEEP_EYE_HEIGHT
        } else {
            target.y + self.camera_step_y_offset + self.camera_sneak_y_offset
        };
        self.cam.pos = Vec3::new(target.x, eye_y, target.z);
        self.last_player_eye_y = target.y;
    }

    /// Keep the REPLICA's view centre (mesh/light priority ordering + the
    /// always-mesh near ring) on the camera, where the streaming target used
    /// to live. Streaming itself is server-side since C2c-ii
    /// (`ServerGame::pump_streaming`); the replica never generates.
    pub(super) fn tick_replica_view(&mut self) {
        let cam_cx = (self.cam.pos.x.floor() as i32).div_euclid(16);
        let cam_cy = (self.cam.pos.y.floor() as i32).div_euclid(16);
        let cam_cz = (self.cam.pos.z.floor() as i32).div_euclid(16);
        self.replica.set_replica_view_center(cam_cx, cam_cy, cam_cz);
    }

    /// Refresh the CLIENT's per-frame targeting: the raycast hit (presentation +
    /// `PlayerUpdate.target` source) against the REPLICA world, the mob
    /// under the crosshair from the REPLICATED rows, and the remote PLAYER
    /// under the crosshair from the remote-player rows (PvP). All three
    /// compete by distance — the nearest wins; a closer block occludes both
    /// entity kinds. At most one of `targeted_mob`/`targeted_player` is set
    /// (the click actions carry them on the wire).
    pub(super) fn refresh_target(&mut self) {
        let block_hit = Player::raycast_with_dist(self.cam.pos, self.cam.forward(), &self.replica);
        self.look = block_hit.map(|(h, _)| h);
        // The use-click target: the held item may declare a water-stopping
        // use ray (a boat item targets the water surface); everything else
        // keeps the selection hit. Mining/selection never read this. The held
        // item comes from the REPLICATED inventory view like every other
        // client held-item decision — `player.inventory` only tracks the
        // active slot index client-side, never contents.
        let held_water_ray = self
            .self_view
            .inventory
            .selected()
            .is_some_and(|st| st.item.use_ray() == crate::item::UseRay::Water);
        self.use_look = if held_water_ray {
            Player::raycast_including_water(self.cam.pos, self.cam.forward(), &self.replica)
                .map(|(h, _)| h)
        } else {
            self.look
        };
        let block_dist = block_hit.map(|(_, d)| d).unwrap_or(player::REACH);
        let mob = self.closest_mob(self.cam.pos, self.cam.forward(), block_dist);
        let remote = self.closest_remote_player(self.cam.pos, self.cam.forward(), block_dist);
        self.targeted_mob = None;
        self.targeted_player = None;
        match (mob, remote) {
            (Some((_, mt)), Some((pid, pt))) if pt < mt => self.targeted_player = Some(pid),
            (Some((id, _)), _) => self.targeted_mob = Some(id),
            (None, Some((pid, _))) => self.targeted_player = Some(pid),
            (None, None) => {}
        }
        if self.targeted_mob.is_some() || self.targeted_player.is_some() {
            self.look = None;
            self.use_look = None;
        }
    }

    /// The stable id of the mob currently under the crosshair — what a click
    /// action carries on the wire.
    pub(super) fn targeted_mob_id(&self) -> Option<u64> {
        self.targeted_mob
    }

    /// The closest replicated mob in front of the eye whose shared body boxes
    /// the ray enters within `max_dist` (and within reach), with its ray
    /// distance; skips dead corpses. `max_dist` is the block hit distance, so
    /// a mob *behind* the block isn't targeted (the block occludes it).
    pub(super) fn closest_mob(&self, eye: Vec3, dir: Vec3, max_dist: f32) -> Option<(u64, f32)> {
        let limit = max_dist.min(player::REACH);
        let own_mount = self.self_mount.map(|(id, _)| id);
        let alpha = self.tick_alpha();
        let bodies = self.replicated_mobs.iter().filter_map(|entry| {
            let row = &entry.curr;
            (!row.dead && Some(row.id) != own_mount).then(|| {
                let (pos, yaw) = entry.interpolated_pose(alpha);
                (
                    row.id,
                    pos,
                    yaw,
                    crate::mob::def(crate::mob::Mob(row.kind_id)).size,
                )
            })
        });
        crate::mob::closest_body_ray_hit(eye, dir, limit, bodies)
    }

    /// The closest VISIBLE, alive remote player whose body AABB (row feet
    /// position + the player half-extents) the ray enters within `max_dist`
    /// (and within reach), with its ray distance — [`closest_mob`] over the
    /// remote-player rows. The store never holds the own id, so self-targeting
    /// is impossible; spectators and the dead ship `visible: false`/`alive:
    /// false` rows and are skipped.
    ///
    /// [`closest_mob`]: Self::closest_mob
    pub(super) fn closest_remote_player(
        &self,
        eye: Vec3,
        dir: Vec3,
        max_dist: f32,
    ) -> Option<(u8, f32)> {
        let limit = max_dist.min(player::REACH);
        let mut best: Option<(u8, f32)> = None;
        for p in self.remote_players.iter() {
            let row = &p.curr;
            if !row.visible || !row.alive {
                continue;
            }
            let pos = row.transform.pos;
            let min = Vec3::new(pos.x - player::HALF_W, pos.y, pos.z - player::HALF_W);
            let max = Vec3::new(
                pos.x + player::HALF_W,
                pos.y + player::HEIGHT,
                pos.z + player::HALF_W,
            );
            if let Some(t) = player::ray_vs_aabb(eye, dir, min, max) {
                if t <= limit && best.is_none_or(|(_, bt)| t < bt) {
                    best = Some((row.id.0, t));
                }
            }
        }
        best
    }
}
