use std::collections::BTreeMap;

use mod_sdk::*;

use crate::keys;

const NUDGE_OFFSETS: [(i32, i32); 25] = [
    (0, 0),
    (-1, 0),
    (0, -1),
    (0, 1),
    (1, 0),
    (-1, -1),
    (-1, 1),
    (1, -1),
    (1, 1),
    (-2, 0),
    (0, -2),
    (0, 2),
    (2, 0),
    (-2, -1),
    (-2, 1),
    (-1, -2),
    (-1, 2),
    (1, -2),
    (1, 2),
    (2, -1),
    (2, 1),
    (-2, -2),
    (-2, 2),
    (2, -2),
    (2, 2),
];

const ACCEL: f32 = 0.26;
const DRAG: f32 = 0.96;
const MAX_FORWARD: f32 = 5.5;
const MAX_REVERSE: f32 = 1.8;
const TURN_ACCEL: f32 = 0.012;
const TURN_DRAG: f32 = 0.85;
const INPUT_EPS: f32 = 0.1;
const REST_SPEED: f32 = 0.01;
const REST_YAW_VEL: f32 = 0.001;

pub(crate) fn wrap_yaw(yaw: f32) -> f32 {
    if !yaw.is_finite() {
        return 0.0;
    }
    let wrapped = yaw.rem_euclid(std::f32::consts::TAU);
    if wrapped > std::f32::consts::PI {
        wrapped - std::f32::consts::TAU
    } else {
        wrapped
    }
}

const STROKE_SECONDS: f32 = 1.5;
const SETTLE_RATE: f32 = 0.75;

struct Boat {
    riders: Vec<PlayerId>,
    speed: f32,
    yaw: f32,
    yaw_vel: f32,
    oars_active: bool,
    oar_steered: [Option<f32>; 2],
}

impl Boat {
    fn new(yaw: f32) -> Self {
        Self {
            riders: Vec::new(),
            speed: 0.0,
            yaw: wrap_yaw(yaw),
            yaw_vel: 0.0,
            oars_active: false,
            oar_steered: [None; 2],
        }
    }
}

#[derive(Default)]
pub struct Boats {
    boat_item: Option<ItemId>,
    boat_kind: Option<MobId>,
    water: Option<BlockId>,
    boats: BTreeMap<u64, Boat>,
}

impl Boats {
    pub fn init(&mut self) {
        self.boat_item = resolve_item_logged(keys::BOAT_ITEM);
        self.boat_kind = resolve_mob_logged(keys::BOAT_MOB);
        self.water = resolve_block_logged(keys::WATER);
    }

    pub fn on_item_use(&mut self, item: ItemId, target: Option<[i32; 3]>) -> Outcome {
        if Some(item) != self.boat_item {
            return Outcome::Continue;
        }
        let (Some(pos), Some(water)) = (target, self.water) else {
            return Outcome::Continue;
        };
        if get_block(pos) != Some(water)
            || get_block([pos[0], pos[1] + 1, pos[2]]) != Some(BlockId::AIR)
        {
            return Outcome::Continue;
        }
        if !consume_held(item, 1) {
            return Outcome::Continue;
        }
        let player = player_state();
        let yaw = player.yaw + std::f32::consts::PI;
        let spawned = NUDGE_OFFSETS.iter().find_map(|&(dx, dz)| {
            let c = [pos[0] + dx, pos[1], pos[2] + dz];
            if (dx != 0 || dz != 0)
                && (get_block(c) != Some(water)
                    || get_block([c[0], c[1] + 1, c[2]]) != Some(BlockId::AIR))
            {
                return None;
            }
            let feet = [c[0] as f64 + 0.5, c[1] as f64 + 0.9, c[2] as f64 + 0.5];
            spawn_mob_checked(keys::BOAT_MOB, feet, yaw)
        });
        if spawned.is_none() {
            give_item(keys::BOAT_ITEM, 1);
        }
        Outcome::Cancel
    }

    pub fn on_interact_mob(&mut self, mob_id: u64, kind: MobId, player: PlayerId) -> Outcome {
        if Some(kind) != self.boat_kind {
            return Outcome::Continue;
        }
        if self.board(mob_id, player) {
            Outcome::Cancel
        } else {
            Outcome::Continue
        }
    }

    pub fn on_dismounted(&mut self, player_id: PlayerId, mount: &MountTarget) {
        if let MountTarget::Mob(mob_id) = mount {
            if let Some(boat) = self.boats.get_mut(mob_id) {
                boat.riders.retain(|p| *p != player_id);
            }
        }
    }

    fn board(&mut self, mob_id: u64, player_id: PlayerId) -> bool {
        let Some(seat) = mob_riders(mob_id).and_then(|seats| seats.first_free_seat()) else {
            return false;
        };
        if !mob_mount(mob_id, player_id, seat) {
            return false;
        }
        let boat = self.boats.entry(mob_id).or_insert_with(|| {
            let p = player_state();
            let yaw = mobs_in_radius(p.pos, 8.0)
                .into_iter()
                .find(|m| m.id == mob_id)
                .map(|m| m.yaw)
                .unwrap_or(0.0);
            Boat::new(yaw)
        });
        boat.riders.push(player_id);
        true
    }

    pub fn tick(&mut self) {
        let mut retired: Vec<u64> = Vec::new();
        for (&id, boat) in self.boats.iter_mut() {
            let input = boat.riders.first().and_then(|&driver| player_input(driver));
            let (forward, strafe) = input.map_or((0.0, 0.0), |i| (i.forward, i.strafe));

            boat.speed = ((boat.speed + forward * ACCEL) * DRAG).clamp(-MAX_REVERSE, MAX_FORWARD);
            boat.yaw_vel = (boat.yaw_vel - strafe * TURN_ACCEL) * TURN_DRAG;
            boat.yaw = wrap_yaw(boat.yaw + boat.yaw_vel);

            let facing = mob_facing_xz(boat.yaw);
            let vel = [facing[0] * boat.speed, facing[1] * boat.speed];
            if !mob_drive(id, vel, Some(boat.yaw)) {
                retired.push(id);
                continue;
            }

            // Oars follow the input. Turning left rows only the right oar, turning right only the
            // left, forward both, reverse both backwards. Let go and it eases back to rest
            // (`steer_oar`). The anims are started once and left running, and we steer them purely
            // through playback.
            if !boat.oars_active {
                boat.oars_active = ensure_oars(id);
            }
            if boat.oars_active {
                let (left_held, right_held) = (strafe < -INPUT_EPS, strafe > INPUT_EPS);
                let forward_held = forward.abs() > INPUT_EPS;
                let dir = if forward < -INPUT_EPS { -1.0 } else { 1.0 };
                let right = if left_held || (forward_held && !right_held) {
                    dir
                } else {
                    0.0
                };
                let left = if right_held || (forward_held && !left_held) {
                    dir
                } else {
                    0.0
                };
                for (slot, (name, desired)) in [("row_left", left), ("row_right", right)]
                    .into_iter()
                    .enumerate()
                {
                    if boat.oar_steered[slot] == Some(desired) {
                        continue;
                    }
                    if !steer_oar(id, name, desired) {
                        deactivate_oars(id);
                        boat.oars_active = false;
                        boat.oar_steered = [None; 2];
                        break;
                    }
                    boat.oar_steered[slot] = Some(desired);
                }
            }

            if boat.riders.is_empty()
                && boat.speed.abs() < REST_SPEED
                && boat.yaw_vel.abs() < REST_YAW_VEL
            {
                retired.push(id);
            }
        }
        for id in retired {
            self.boats.remove(&id);
        }
    }
}

fn ensure_oars(id: u64) -> bool {
    match (
        mob_anim_state(id, "row_left"),
        mob_anim_state(id, "row_right"),
    ) {
        (Some(_), Some(_)) => true,
        (None, None) => activate_parked_oars(id),
        _ => {
            deactivate_oars(id);
            activate_parked_oars(id)
        }
    }
}

fn activate_parked_oars(id: u64) -> bool {
    let left_active = mob_anim_set(id, "row_left", true);
    let right_active = left_active && mob_anim_set(id, "row_right", true);
    if !right_active {
        if left_active {
            mob_anim_set(id, "row_left", false);
        }
        return false;
    }

    let left_parked = mob_anim_rate(id, "row_left", 0.0);
    let right_parked = mob_anim_rate(id, "row_right", 0.0);
    if left_parked && right_parked {
        true
    } else {
        deactivate_oars(id);
        false
    }
}

fn deactivate_oars(id: u64) {
    mob_anim_set(id, "row_left", false);
    mob_anim_set(id, "row_right", false);
}

fn steer_oar(id: u64, name: &str, desired: f32) -> bool {
    let Some(state) = mob_anim_state(id, name) else {
        return false;
    };
    if desired != 0.0 {
        if state.seek.is_some() || state.rate != desired {
            return mob_anim_rate(id, name, desired);
        }
    } else if state.seek.is_none() && state.rate != 0.0 {
        let target = (state.phase / STROKE_SECONDS).round() * STROKE_SECONDS;
        return mob_anim_seek(id, name, target, SETTLE_RATE);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaw_wrap_stays_finite_and_bounded_during_long_running_integration() {
        for yaw in [f32::MAX, -f32::MAX, f32::INFINITY, f32::NAN] {
            let wrapped = wrap_yaw(yaw);
            assert!(wrapped.is_finite());
            assert!(wrapped.abs() <= std::f32::consts::PI);
        }

        let mut yaw = 0.0;
        for _ in 0..100_000 {
            yaw = wrap_yaw(yaw + 1.234_567);
        }
        assert!(yaw.is_finite());
        assert!(yaw.abs() <= std::f32::consts::PI);
    }
}
