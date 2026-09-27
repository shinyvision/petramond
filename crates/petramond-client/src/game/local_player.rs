use petramond::net::protocol::SelfTransform;
use petramond::player::{self, Input, Player, RaycastHit};
use petramond_math::math::Vec3;
use petramond_render::camera::Camera;
use petramond_world::mining::MiningState;
use petramond_world::world::placement::HeldRotation;
use petramond_world::world::raycast;

use super::camera_rig::{CameraRig, EyeInputs, STEP_CAMERA_EPS};
use super::creative::{BreakRepeat, FlightToggle};
use super::replica_state::ReplicaState;
use super::third_person::ThirdPerson;
use super::{Game, GameInput};

const LOOK_SENSITIVITY: f32 = 0.0025;

pub(super) struct LocalPlayer {
    pub(super) player: Player,
    pub(super) cam: Camera,
    pub(super) camera_rig: CameraRig,
    pub(super) third_person: ThirdPerson,
    pub(super) look: Option<RaycastHit>,
    pub(super) use_look: Option<RaycastHit>,
    pub(super) targeted_mob: Option<u64>,
    pub(super) targeted_player: Option<u8>,
    pub(super) held_rotation: HeldRotation,
    pub(super) predicted_input: Input,
    pub(super) intent_use_held: bool,
    pub(super) last_sent_transform: Option<SelfTransform>,
    pub(super) mining: MiningState,
    pub(super) flight_toggle: FlightToggle,
    pub(super) break_repeat: BreakRepeat,
}

impl LocalPlayer {
    pub(super) fn new(mut cam: Camera, player: Player) -> Self {
        cam.pos = player.eye();
        cam.yaw = player.yaw;
        cam.pitch = player.pitch;
        let camera_rig = CameraRig::new(cam.fov_y, player.eye().y);
        Self {
            player,
            cam,
            camera_rig,
            third_person: ThirdPerson::default(),
            look: None,
            use_look: None,
            targeted_mob: None,
            targeted_player: None,
            held_rotation: HeldRotation::default(),
            predicted_input: Input::default(),
            intent_use_held: false,
            last_sent_transform: None,
            mining: MiningState::new(),
            flight_toggle: FlightToggle::default(),
            break_repeat: BreakRepeat::default(),
        }
    }

    fn apply_look(&mut self, (dx, dy): (f32, f32)) {
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        self.player
            .rotate(-dx * LOOK_SENSITIVITY, -dy * LOOK_SENSITIVITY);
        self.cam.yaw = self.player.yaw;
        self.cam.pitch = self.player.pitch;
    }

    fn scroll_hotbar(&mut self, scroll: i32) -> u8 {
        let before = self.player.inventory.active_slot();
        self.player.inventory.scroll_active(scroll);
        let after = self.player.inventory.active_slot();
        if after != before {
            self.held_rotation.clear();
        }
        after
    }

    fn movement_intent(&self, input: &GameInput) -> Input {
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
            let m = &input.movement;
            for (held, dir) in [
                (m.forward, fwd),
                (m.backward, -fwd),
                (m.right, right),
                (m.left, -right),
                (spectator && m.jump, Vec3::Y),
                (spectator && m.sneak, -Vec3::Y),
            ] {
                if held {
                    wishdir += dir;
                }
            }
        }
        Input {
            wishdir: wishdir.normalize_or_zero(),
            jump: input.gameplay_enabled && input.movement.jump,
            sprint: input.gameplay_enabled && input.movement.sprint,
            sneak: input.gameplay_enabled && input.movement.sneak,
        }
    }

    fn simulate(&mut self, replica: &ReplicaState, dt: f32, input: &GameInput) {
        let player_input = self.movement_intent(input);
        self.predicted_input = player_input;
        self.intent_use_held = input.use_held && input.gameplay_enabled;
        if !self.intent_use_held {
            self.player.use_gesture = player::UseGesture::Free;
        }
        self.player.refresh_engine_claims(input.gameplay_enabled);

        let ratio = self.player.wish_speed(player_input) / player::WALK;
        self.cam.fov_y =
            self.camera_rig
                .advance_lens(dt, ratio, self.player.yaw, self.player.pitch);

        if let Some(seat_pos) = replica.self_mount_pose().map(|m| m.seat) {
            self.player.pos = seat_pos;
            self.player.vel = Vec3::ZERO;
            self.player.on_ground = true;
            self.sync_camera_to_eye(replica, dt);
            return;
        }

        if self.player.is_spectator() || self.player.columns_loaded(replica.world.data()) {
            let obstacles = replica.solid_entity_obstacles();
            let mut remaining = dt.min(0.25);
            while remaining > 0.0 {
                let step = remaining.min(player::DT_MAX);
                self.player.update_with_obstacles(
                    step,
                    replica.world.data(),
                    player_input,
                    &obstacles,
                );
                remaining -= step;
            }
        }

        self.sync_camera_to_eye(replica, dt);
    }

    fn apply_entity_push(&mut self, replica: &ReplicaState, dt: f32) {
        if self.player.is_spectator() || replica.entities.own_mount().is_some() {
            return;
        }
        let body = self.player.body();
        let mut push = Vec3::ZERO;
        for entry in replica.entities.mobs().iter() {
            if entry.curr.dead {
                continue;
            }
            let d = petramond::mob::def(petramond::mob::Mob(entry.curr.kind_id));
            if d.collision == petramond::mob::MobCollision::Solid {
                continue;
            }
            let mob =
                petramond_world::body::Body::new(entry.curr.pos, d.size.half_width, d.size.height);
            if let Some(p) = petramond_world::body::separation(body, mob) {
                push += p;
            }
        }
        for remote in replica.entities.players().iter() {
            let Some(other) = remote.push_body() else {
                continue;
            };
            if let Some(p) = petramond_world::body::separation(body, other) {
                push += p;
            }
        }
        if push != Vec3::ZERO {
            self.player.shove(push * dt, replica.world.data());
            self.sync_camera_to_eye(replica, dt);
        }
    }

    fn sync_camera_to_eye(&mut self, replica: &ReplicaState, dt: f32) {
        let mounted = replica.entities.own_mount().is_some();
        let spectator = self.player.is_spectator();
        let sleeping = replica.self_view.sleeping.is_some();
        let body = EyeInputs {
            eye: self.player.eye(),
            feet_y: self.player.pos.y,
            grounded_still: self.player.on_ground && self.player.vel.y.abs() <= STEP_CAMERA_EPS,
            carried: spectator || mounted,
            sneaking: !spectator && self.predicted_input.sneak,
            // A sleeper, rider, spectator or airborne body eases the sway back
            // to rest. THIRD PERSON DOES NOT BOB: the boom camera is `cam`
            // cloned and retreated (`update_third_person`, which runs after
            // this), so suppressing the sway here is what keeps it out of the
            // boom; easing to rest rather than skipping the apply means
            // toggling back mid-stride eases the sway in.
            striding: self.player.on_ground
                && !spectator
                && !mounted
                && !sleeping
                && !self.third_person.enabled,
            hspeed: Vec3::new(self.player.vel.x, 0.0, self.player.vel.z).length(),
            sleeping,
            yaw: self.cam.yaw,
        };
        self.cam.pos = self.camera_rig.place_eye(dt, body);
    }

    fn refresh_target(&mut self, replica: &ReplicaState) {
        let eye = self.cam.pos;
        let dir = self.cam.forward();
        let block_hit = raycast::with_dist(eye, dir, replica.world.data());
        self.look = block_hit.map(|(h, _)| h);
        let fluid_ray = |st: Option<&petramond_world::item::ItemStack>| {
            st.map(|st| st.item.use_ray())
                .filter(|ray| ray.sees_fluid())
        };
        let inventory = &replica.self_view.inventory;
        let held_fluid_ray =
            fluid_ray(inventory.selected()).or_else(|| fluid_ray(inventory.off_hand()));
        self.use_look = match held_fluid_ray {
            Some(ray) => raycast::use_ray(eye, dir, replica.world.data(), ray).map(|(h, _)| h),
            None => self.look,
        };
        let block_dist = block_hit.map(|(_, d)| d).unwrap_or(player::REACH);
        let mob = replica.closest_mob(eye, dir, block_dist);
        let remote = replica.closest_remote_player(eye, dir, block_dist);
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

    fn view_section(&self) -> (i32, i32, i32) {
        let section = |v: f64| (v.floor() as i32).div_euclid(16);
        (
            section(self.cam.pos.x),
            section(self.cam.pos.y),
            section(self.cam.pos.z),
        )
    }
}

impl Game {
    pub(super) fn apply_camera_input(&mut self, input: &GameInput) {
        if !input.gameplay_enabled || self.world_tool_holds_camera() {
            return;
        }
        self.local.apply_look(input.look_delta);
    }

    pub(super) fn apply_hotbar_input(&mut self, input: &GameInput) {
        if input.gameplay_enabled && input.hotbar_scroll != 0 && !self.in_presentation() {
            let slot = self.local.scroll_hotbar(input.hotbar_scroll);
            self.replica.self_view.inventory.set_active(slot);
        }
    }

    pub(super) fn tick_player(&mut self, dt: f32, input: &GameInput) {
        self.local.simulate(&self.replica, dt, input);
        if !self.local.intent_use_held {
            self.client_mods.release_use();
        }
    }

    pub(super) fn apply_entity_push(&mut self, dt: f32) {
        self.local.apply_entity_push(&self.replica, dt);
    }

    pub(super) fn sync_camera_to_player_eye(&mut self, dt: f32) {
        self.local.sync_camera_to_eye(&self.replica, dt);
    }

    pub(super) fn tick_replica_view(&mut self) {
        let (cx, cy, cz) = self.local.view_section();
        self.replica.world.set_replica_view_center(cx, cy, cz);
    }

    pub(super) fn refresh_target(&mut self) {
        self.local.refresh_target(&self.replica);
    }

    pub(super) fn targeted_mob_id(&self) -> Option<u64> {
        self.local.targeted_mob
    }
}

#[cfg(test)]
mod tests;
