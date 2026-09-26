//! The client's locally-simulated player: per-frame look/movement physics on
//! `Game::player`, the camera mirror, and the per-frame target refresh
//! (`Game::look`/`Game::targeted_mob`). None of this touches the sessions —
//! the results reach the sim as the next `PlayerUpdate` message.

use petramond::player::{self, Input, Player};
use petramond_math::math::Vec3;

use super::camera_rig::{EyeInputs, STEP_CAMERA_EPS};
use super::replicated::MountPose;
use super::{Game, GameInput};

impl Game {
    pub(super) fn apply_camera_input(&mut self, input: &GameInput) {
        if !input.gameplay_enabled || self.world_tool_holds_camera() {
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
        // The gameplay-gated use intent this frame — the client twin of the
        // server's `sess.using()` (the snapshot's `use_held`, and what the
        // wire ships in `PlayerUpdate`).
        self.intent_use_held = input.use_held && input.gameplay_enabled;
        // LETTING GO ends the gesture, here as on the server. Every hold is
        // released together: whoever had the press, they no longer do.
        if !self.intent_use_held {
            self.client_mods.release_use();
            self.player.use_gesture = petramond::player::UseGesture::Free;
        }
        // The engine's own half of this body's claims, re-derived here rather
        // than waited for: every input is state the client already holds, so
        // the effect slow and the menu that takes the hands stay PREDICTED
        // instead of arriving a batch after the player felt them.
        self.player.refresh_engine_claims(input.gameplay_enabled);

        // Speed-coupled FOV: the camera eases toward the WISHED land speed —
        // the same number the physics below applies — so sprint (only while
        // deliberately moving; a held key over planted feet wishes nothing),
        // a speed effect (moving or not — it is body state, not a wish), and
        // both combined all arrive through one signal with no per-cause
        // checks. Before the mount early-return on purpose: the widening
        // follows the body's own selection wherever the body happens to be.
        let ratio = self.player.wish_speed(player_input) / player::WALK;
        self.cam.fov_y =
            self.camera_rig
                .advance_lens(dt, ratio, self.player.yaw, self.player.pitch);

        // Mounted: no local physics — the body slaves to the interpolated
        // mount at the seat offset, the same glue observers apply to mounted
        // remotes, so rider and mount can never visibly separate. The intent
        // stashed above still rides `PlayerUpdate` (that IS the steering
        // input the driving mod reads server-side).
        if let Some(seat_pos) = self.self_mount_pose().map(|m| m.seat) {
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
    pub(super) fn solid_entity_obstacles(&self) -> Vec<petramond_world::collision::DynBox> {
        let alpha = self.tick_alpha();
        let own_mount = self.entities.own_mount().and_then(|m| match m {
            petramond::net::protocol::PlayerMount::Mob { id, .. } => Some(id),
            petramond::net::protocol::PlayerMount::Anchor { .. } => None,
        });
        let mut out = Vec::new();
        for entry in self.entities.mobs().iter() {
            let row = &entry.curr;
            if row.dead || Some(row.id) == own_mount {
                continue;
            }
            let d = petramond::mob::def(petramond::mob::Mob(row.kind_id));
            if d.collision != petramond::mob::MobCollision::Solid {
                continue;
            }
            let (pos, yaw) = entry.interpolated_pose(alpha);
            petramond::mob::solid_boxes(row.id, pos, yaw, d.size, &mut out);
        }
        out
    }

    /// The local player's frame on its mount this frame: `None` when
    /// unmounted, or while a mob mount's rows are not available yet — the
    /// caller keeps its current transform and waits for the rows to agree.
    /// A rider sits square in its seat and leans with it; only the head
    /// follows the look (see `collect_player`).
    pub(super) fn self_mount_pose(&self) -> Option<MountPose> {
        self.entities.mobs()
            .mount_pose(self.entities.own_mount()?, self.tick_alpha())
    }

    /// Per-frame push of the player out of overlapping soft bodies (mobs +
    /// remote players). Applied as a push *velocity* over this frame's `dt` so
    /// it never accumulates or fights the movement controller. Each client
    /// only ever shoves ITSELF (the shove rides its next `PlayerUpdate`), so
    /// player↔player separation stays symmetric with no server-side push step;
    /// the mobs' own half runs on the tick (`game_tick_step`).
    pub(super) fn apply_entity_push(&mut self, dt: f32) {
        // A mounted body is slaved to its seat: nothing may jostle it (its
        // own mount overlaps it every frame).
        if self.player.is_spectator() || self.entities.own_mount().is_some() {
            return;
        }
        let body = self.player.body();
        let mut push = Vec3::ZERO;
        for entry in self.entities.mobs().iter() {
            if entry.curr.dead {
                continue; // a ragdolling corpse doesn't push
            }
            let d = petramond::mob::def(petramond::mob::Mob(entry.curr.kind_id));
            if d.collision == petramond::mob::MobCollision::Solid {
                // Rigid geometry in the player's own resolver — a soft push
                // on top would fight the contact (skating a deck-stander).
                continue;
            }
            let mob =
                petramond_world::body::Body::new(entry.curr.pos, d.size.half_width, d.size.height);
            if let Some(p) = petramond_world::body::separation(body, mob) {
                push += p;
            }
        }
        for remote in self.entities.players().iter() {
            let Some(other) = remote.push_body() else {
                continue; // hidden (spectator/dead) or asleep in a bed
            };
            if let Some(p) = petramond_world::body::separation(body, other) {
                push += p;
            }
        }
        if push != Vec3::ZERO {
            self.player.shove(push * dt, &self.replica);
            self.sync_camera_to_player_eye(dt);
        }
    }

    pub(super) fn sync_camera_to_player_eye(&mut self, dt: f32) {
        let mounted = self.entities.own_mount().is_some();
        let spectator = self.player.is_spectator();
        let sleeping = self.self_view.sleeping.is_some();
        let body = EyeInputs {
            eye: self.player.eye(),
            feet_y: self.player.pos.y,
            grounded_still: self.player.on_ground && self.player.vel.y.abs() <= STEP_CAMERA_EPS,
            carried: spectator || mounted,
            sneaking: !spectator && self.predicted_input.sneak,
            // A sleeper, rider, spectator or airborne body eases the sway back
            // to rest. THIRD PERSON DOES NOT BOB: the boom camera is `self.cam`
            // cloned and retreated (`update_third_person`, which runs after
            // this), so suppressing the sway here is what keeps it out of the
            // boom; easing to rest rather than skipping the apply means
            // toggling back mid-stride eases the sway in.
            striding: self.player.on_ground
                && !spectator
                && !mounted
                && !sleeping
                && !self.third_person_enabled(),
            hspeed: Vec3::new(self.player.vel.x, 0.0, self.player.vel.z).length(),
            sleeping,
            // `cam.yaw` is already this frame's look (mirrored in `apply_look`).
            yaw: self.cam.yaw,
        };
        self.cam.pos = self.camera_rig.place_eye(dt, body);
    }

    /// Keep the REPLICA's view centre (mesh/light priority ordering + the
    /// always-mesh near ring) on the camera. Streaming itself is server-side
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
        // The use-click target: a held item may declare a fluid-stopping use
        // ray (a boat item targets the water surface); everything else keeps
        // the selection hit. Mining/selection never read this. EITHER hand's
        // fluid-ray item selects its ray — the server's
        // `PendingUseClick::ray_item` rule, so an off-hand boat gets the fluid
        // target its second dispatch pass will act on. The held items come
        // from the REPLICATED inventory view like every other client
        // held-item decision — `player.inventory` only tracks the active slot
        // index client-side, never contents.
        let fluid_ray = |st: Option<&petramond_world::item::ItemStack>| {
            st.map(|st| st.item.use_ray())
                .filter(|ray| ray.sees_fluid())
        };
        let held_fluid_ray = fluid_ray(self.self_view.inventory.selected())
            .or_else(|| fluid_ray(self.self_view.inventory.off_hand()));
        self.use_look = match held_fluid_ray {
            Some(ray) => {
                Player::raycast_use_ray(self.cam.pos, self.cam.forward(), &self.replica, ray)
                    .map(|(h, _)| h)
            }
            None => self.look,
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
    pub(super) fn closest_mob(
        &self,
        eye: petramond_math::world_pos::WorldPos,
        dir: Vec3,
        max_dist: f32,
    ) -> Option<(u64, f32)> {
        let limit = max_dist.min(player::REACH);
        let own_mount = self.entities.own_mount().and_then(|m| match m {
            petramond::net::protocol::PlayerMount::Mob { id, .. } => Some(id),
            petramond::net::protocol::PlayerMount::Anchor { .. } => None,
        });
        let alpha = self.tick_alpha();
        let bodies = self.entities.mobs().iter().filter_map(|entry| {
            let row = &entry.curr;
            (!row.dead && Some(row.id) != own_mount).then(|| {
                let (pos, yaw) = entry.interpolated_pose(alpha);
                (
                    row.id,
                    pos,
                    yaw,
                    petramond::mob::def(petramond::mob::Mob(row.kind_id)).size,
                )
            })
        });
        petramond::mob::closest_body_ray_hit(eye, dir, limit, bodies)
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
        eye: petramond_math::world_pos::WorldPos,
        dir: Vec3,
        max_dist: f32,
    ) -> Option<(u8, f32)> {
        let limit = max_dist.min(player::REACH);
        let mut best: Option<(u8, f32)> = None;
        for p in self.entities.players().iter() {
            let row = &p.curr;
            if !row.visible || !row.alive {
                continue;
            }
            let pos = row.transform.pos - eye;
            let min = Vec3::new(pos.x - player::HALF_W, pos.y, pos.z - player::HALF_W);
            let max = Vec3::new(
                pos.x + player::HALF_W,
                pos.y + player::HEIGHT,
                pos.z + player::HALF_W,
            );
            if let Some(t) = player::ray_vs_aabb(Vec3::ZERO, dir, min, max) {
                if t <= limit && best.is_none_or(|(_, bt)| t < bt) {
                    best = Some((row.id.0, t));
                }
            }
        }
        best
    }
}
