//! The client's locally-simulated player as one owned subsystem: per-frame
//! look/movement physics on [`LocalPlayer::player`], the camera and its
//! easing, the third-person view, per-frame targeting and the input intents
//! the next `PlayerUpdate` carries. None of this touches the sessions — the
//! results reach the sim as the next `PlayerUpdate` message.
//!
//! Every world read goes through the [`ReplicaState`] borrow `Game` passes
//! in; `Game` keeps only thin coordinator entry points that combine this
//! subsystem with the others (the tools' camera hold, client-mod use holds).

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

/// Mouse-look sensitivity (radians per raw mouse count).
const LOOK_SENSITIVITY: f32 = 0.0025;

pub(super) struct LocalPlayer {
    /// The client's LOCALLY-SIMULATED player: movement physics runs on this
    /// copy every frame and the camera mirrors its eye. Its transform is sent
    /// to the server in each frame's `PlayerUpdate` (trusted verbatim for the
    /// local session); server-side transform mutations (teleports, knockback)
    /// are adopted back after the fixed ticks. Its INVENTORY CONTENTS are a
    /// stale clone — only the active-slot index is meaningful client-side; the
    /// authoritative inventory lives on the session.
    pub(super) player: Player,
    /// The authoritative first-person eye every presentation consumer reads
    /// (raycast, streaming, audio, reach). The third-person boom camera is
    /// derived from it, never the other way round.
    pub(super) cam: Camera,
    /// The first-person camera's presentation easing (step glide, sneak dip,
    /// walking sway, pillow eye, speed FOV).
    pub(super) camera_rig: CameraRig,
    /// Third-person view state (boom camera + body pose). `cam` above stays
    /// the authoritative first-person eye; see `third_person.rs`.
    pub(super) third_person: ThirdPerson,
    /// The client's per-frame raycast target: presentation (selection
    /// outline, mining dust) + the `PlayerUpdate.target` message source.
    /// `None` when a mob or player is the closer target.
    pub(super) look: Option<RaycastHit>,
    /// The USE-click target this frame: equal to [`look`](Self::look) unless
    /// a held item declares a water-stopping use ray (`use_ray: water` in
    /// `items.json`), in which case the first water cell in reach can be the
    /// target. Rides `UseClick.target` only — selection outline, mining, and
    /// the look latch keep the normal water-transparent ray.
    pub(super) use_look: Option<RaycastHit>,
    /// The mob under the crosshair this frame (STABLE replicated id), nearer
    /// than any block. At most one of `targeted_mob`/`targeted_player` is set;
    /// the click actions carry it on the wire.
    pub(super) targeted_mob: Option<u64>,
    /// The remote PLAYER under the crosshair this frame (`PlayerId` byte),
    /// nearer than any block or mob — the PvP attack target.
    pub(super) targeted_player: Option<u8>,
    /// The client-owned R-key placement-rotation cycle; its raw counter rides
    /// `PlayerUpdate.held_rotation` (the session keeps its own latched copy).
    pub(super) held_rotation: HeldRotation,
    /// The movement `Input` this frame's local physics consumed
    /// ([`Game::tick_player`]) — reused verbatim by `build_player_update` so
    /// the wire intent can never drift from what the prediction simulated.
    pub(super) predicted_input: Input,
    /// The gameplay-gated use (interact) button intent this frame — the
    /// client twin of the server's `sess.using()`; feeds the client actor
    /// snapshot's `use_held` for mod predictors.
    pub(super) intent_use_held: bool,
    /// The transform of the last `PlayerUpdate` this client SENT. A
    /// `SelfState::transform` correction adopts only the fields that differ
    /// from it: fields equal to what we last claimed are just the server
    /// echoing us, and the local (possibly newer) value wins.
    pub(super) last_sent_transform: Option<SelfTransform>,
    /// Local mining timer for crack overlay + `BreakFinished` (P2).
    pub(super) mining: MiningState,
    /// Creative double-jump flight toggling.
    pub(super) flight_toggle: FlightToggle,
    /// Creative instant-break repeat pacing while the button is held.
    pub(super) break_repeat: BreakRepeat,
}

impl LocalPlayer {
    /// The local player restored from the join, with the camera placed at its
    /// eye. The camera the caller built carries the authored FOV; the
    /// per-frame speed widening multiplies on top of it (see `CameraRig`).
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

    /// Turn the body by a raw mouse delta and mirror the look onto the camera
    /// now, before this frame's movement and raycast read `cam.forward()`.
    fn apply_look(&mut self, (dx, dy): (f32, f32)) {
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        self.player
            .rotate(-dx * LOOK_SENSITIVITY, -dy * LOOK_SENSITIVITY);
        self.cam.yaw = self.player.yaw;
        self.cam.pitch = self.player.pitch;
    }

    /// Scroll the client-owned hotbar selection (only the INDEX matters —
    /// contents are session-owned); the slot rides `PlayerUpdate.hotbar_slot`.
    /// Any hotbar change resets the R-key rotation cycle so the raw wire
    /// counter unambiguously means "R pressed on the current selection".
    /// Returns the selected slot.
    fn scroll_hotbar(&mut self, scroll: i32) -> u8 {
        let before = self.player.inventory.active_slot();
        self.player.inventory.scroll_active(scroll);
        let after = self.player.inventory.active_slot();
        if after != before {
            self.held_rotation.clear();
        }
        after
    }

    /// This frame's movement intent from the gameplay-gated input, relative to
    /// the camera: flat for a walker, full 3D (plus jump/sneak as up/down) for
    /// a spectator.
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

    /// One frame of local movement: latch the intents, re-derive the body's
    /// engine claims, ease the speed FOV, then either slave to the mount's
    /// seat or integrate physics against the replica (and its solid
    /// entities), and finally place the eye.
    fn simulate(&mut self, replica: &ReplicaState, dt: f32, input: &GameInput) {
        let player_input = self.movement_intent(input);
        // Stash for `build_player_update`: the wire intent must be the exact
        // input the local physics consumed this frame.
        self.predicted_input = player_input;
        // The gameplay-gated use intent this frame — the client twin of the
        // server's `sess.using()` (the snapshot's `use_held`, and what the
        // wire ships in `PlayerUpdate`).
        self.intent_use_held = input.use_held && input.gameplay_enabled;
        // LETTING GO ends the gesture, here as on the server.
        if !self.intent_use_held {
            self.player.use_gesture = player::UseGesture::Free;
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
        if let Some(seat_pos) = replica.self_mount_pose().map(|m| m.seat) {
            self.player.pos = seat_pos;
            self.player.vel = Vec3::ZERO;
            self.player.on_ground = true;
            self.sync_camera_to_eye(replica, dt);
            return;
        }

        // Physics gates on the REPLICA's loaded columns: until the spawn area's
        // payloads land, the player holds still (exactly the fresh-world
        // stream-in wait; absent-Mixed sections would read as air and lie).
        if self.player.is_spectator() || self.player.columns_loaded(replica.world.data()) {
            // Solid entities (a boat's hull) block the predicted body exactly
            // like the server's integration does — sourced from the
            // interpolated replicated rows, the same transform they render at.
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

    /// Per-frame push of the player out of overlapping soft bodies (mobs +
    /// remote players). Applied as a push *velocity* over this frame's `dt` so
    /// it never accumulates or fights the movement controller. Each client
    /// only ever shoves ITSELF (the shove rides its next `PlayerUpdate`), so
    /// player↔player separation stays symmetric with no server-side push step;
    /// the mobs' own half runs on the tick (`game_tick_step`).
    fn apply_entity_push(&mut self, replica: &ReplicaState, dt: f32) {
        // A mounted body is slaved to its seat: nothing may jostle it (its
        // own mount overlaps it every frame).
        if self.player.is_spectator() || replica.entities.own_mount().is_some() {
            return;
        }
        let body = self.player.body();
        let mut push = Vec3::ZERO;
        for entry in replica.entities.mobs().iter() {
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
        for remote in replica.entities.players().iter() {
            let Some(other) = remote.push_body() else {
                continue; // hidden (spectator/dead) or asleep in a bed
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

    /// Place the first-person eye through the camera rig's easing (step
    /// glide, sneak dip, walking sway, pillow eye).
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
            // `cam.yaw` is already this frame's look (mirrored in `apply_look`).
            yaw: self.cam.yaw,
        };
        self.cam.pos = self.camera_rig.place_eye(dt, body);
    }

    /// Refresh the CLIENT's per-frame targeting: the raycast hit (presentation
    /// + `PlayerUpdate.target` source) against the replica world, the mob
    ///
    /// The mob under the crosshair comes from the REPLICATED rows; the remote PLAYER
    /// under the crosshair from the remote-player rows (PvP). All three
    /// compete by distance — the nearest wins; a closer block occludes both
    /// entity kinds. At most one of `targeted_mob`/`targeted_player` is set
    /// (the click actions carry them on the wire).
    fn refresh_target(&mut self, replica: &ReplicaState) {
        let eye = self.cam.pos;
        let dir = self.cam.forward();
        let block_hit = raycast::with_dist(eye, dir, replica.world.data());
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

    /// The replica section the camera is in — the replica's view centre
    /// (mesh/light priority ordering + the always-mesh near ring).
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
        if input.gameplay_enabled && input.hotbar_scroll != 0 {
            let slot = self.local.scroll_hotbar(input.hotbar_scroll);
            // Mirror into the replicated view (selection is client-owned;
            // the server never echoes it back).
            self.replica.self_view.inventory.set_active(slot);
        }
    }

    pub(super) fn tick_player(&mut self, dt: f32, input: &GameInput) {
        self.local.simulate(&self.replica, dt, input);
        // Letting go of use releases every client-mod hold together: whoever
        // had the press, they no longer do.
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

    /// Keep the REPLICA's view centre on the camera. Streaming itself is
    /// server-side (`ServerGame::pump_streaming`); the replica never
    /// generates.
    pub(super) fn tick_replica_view(&mut self) {
        let (cx, cy, cz) = self.local.view_section();
        self.replica.world.set_replica_view_center(cx, cy, cz);
    }

    pub(super) fn refresh_target(&mut self) {
        self.local.refresh_target(&self.replica);
    }

    /// The stable id of the mob currently under the crosshair — what a click
    /// action carries on the wire.
    pub(super) fn targeted_mob_id(&self) -> Option<u64> {
        self.local.targeted_mob
    }
}

#[cfg(test)]
mod tests;
