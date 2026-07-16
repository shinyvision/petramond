//! vehicles — rideable vehicles over the generic riding/drive API, starting
//! with a craftable wooden boat.
//!
//! Everything here is POLICY over engine mechanisms; the pack ships no
//! vehicle-specific engine code:
//!
//! - **Placing** (`item_use_pre`): the boat item's `use_ray: water` row makes
//!   its use click target the water surface; a click on a water cell with air
//!   above spawns the `vehicles:boat` mob (bow away from the player) and
//!   consumes the item (`consume_held`). The boat floats on the engine's own
//!   mob buoyancy.
//! - **Boarding** (`mob_interact`): a use click on the boat seats the player
//!   in the first free seat (`mob_mount`; the row declares two). The FIRST
//!   player aboard steers; the engine's sneak gesture (or death, or the boat
//!   sinking out of existence) dismounts, and `player_dismounted` keeps the
//!   rider list honest — the next-oldest rider inherits the oars.
//! - **Rowing** (tick system, `Before(Mobs)`): the driver's `player_input`
//!   forward/strafe feed a small momentum model — acceleration with drag on
//!   speed AND yaw rate, so the boat surges, glides, and carves instead of
//!   jolting — issued to the engine as one `mob_drive` intent per tick.
//!   Collision, gravity, and buoyancy stay engine physics.
//! - **Oars** (`mob_anim_set` + `mob_anim_rate`/`mob_anim_seek`): the
//!   model's looping `row_left`/`row_right` animations activate once (parked
//!   on their authored rest pose — boarding never starts a stroke) and stay
//!   active; authoritative `mob_anim_state` readback drives the control
//!   policy. Forward rows both (rate 1);
//!   turning left rows only the RIGHT oar and vice versa; backing water rows
//!   in reverse (rate −1). A RELEASED oar settles gently back onto its
//!   authored rest pose by the SHORTEST path (a seek to the nearest whole
//!   stroke — never a snap, never the long way around) — a real boat's
//!   asymmetric stroke, code-driven over Blockbench-tunable clips.
//! - **Breaking**: pure engine content — the boat is a 4-health mob, so
//!   punching it runs the ordinary damage pipeline and its loot table drops
//!   the boat item back.
//!
//! Boat state (rider order, speed, yaw rate) is transient mod state keyed by
//! the STABLE mob id: it exists only while someone rides or the hull still
//! glides, and a boat that despawns/unloads/dies simply stops answering
//! `mob_drive`, which retires its entry. A reloaded boat starts at rest.

use std::collections::BTreeMap;

use mod_sdk::*;

const TICK_DRIVE: u32 = 1;
const ON_ITEM_USE: u32 = 1;
const ON_MOB_INTERACT: u32 = 2;
const ON_DISMOUNTED: u32 = 3;

const BOAT_KEY: &str = "vehicles:boat";

/// Forward acceleration per tick of full input (m/s per tick at 20 TPS).
const ACCEL: f32 = 0.26;
/// Per-tick speed retention — the water drag that makes the boat surge up to
/// speed and glide to a stop instead of jolting.
const DRAG: f32 = 0.96;
const MAX_FORWARD: f32 = 5.5;
const MAX_REVERSE: f32 = 1.8;
/// Yaw-rate acceleration per tick of full rudder (radians/tick²) and its
/// per-tick retention — the turning friction: heading carves in and eases out.
const TURN_ACCEL: f32 = 0.012;
const TURN_DRAG: f32 = 0.85;
/// Input dead zone (the wish components are ±1 or ±0.707 on diagonals).
const INPUT_EPS: f32 = 0.1;
/// Below these residuals an unmanned hull counts as at rest and its state
/// retires.
const REST_SPEED: f32 = 0.01;
const REST_YAW_VEL: f32 = 0.001;

fn wrap_yaw(yaw: f32) -> f32 {
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

/// Authored length of one `row_left`/`row_right` stroke in the boat model —
/// MUST match the clip length in `boat.bbmodel` (retune both together): the
/// settle target is the nearest whole multiple of this.
const STROKE_SECONDS: f32 = 1.5;
/// How fast a released oar eases back onto its rest pose (anim-seconds per
/// second) — gentler than the rowing rate of 1.
const SETTLE_RATE: f32 = 0.75;

/// One live boat's transient state, keyed by stable mob id.
struct Boat {
    /// Riders in boarding order; the first steers.
    riders: Vec<u8>,
    /// Signed speed along the bow (m/s); negative = backing water.
    speed: f32,
    /// Facing in the MOB yaw convention (yaw 0 faces -Z), integrated here and
    /// issued back through `mob_drive` — seeded from the live mob at first
    /// boarding.
    yaw: f32,
    yaw_vel: f32,
    /// Whether the two oar animations have been activated on the mob (done
    /// once at first control; thereafter only playback changes).
    oars_active: bool,
}

impl Boat {
    fn new(yaw: f32) -> Self {
        Self {
            riders: Vec::new(),
            speed: 0.0,
            yaw: wrap_yaw(yaw),
            yaw_vel: 0.0,
            oars_active: false,
        }
    }
}

#[derive(Default)]
struct Vehicles {
    boat_item: Option<ItemId>,
    water: Option<BlockId>,
    /// Live boat state by STABLE mob id — `BTreeMap` so the drive tick
    /// iterates deterministically.
    boats: BTreeMap<u64, Boat>,
}

impl Mod for Vehicles {
    fn init(&mut self) {
        self.boat_item = resolve_item_logged(BOAT_KEY);
        self.water = resolve_block_logged("petramond:water");
        register_event_handler(EventKind::ItemUsePre, 0, ON_ITEM_USE);
        register_event_handler(EventKind::MobInteract, 0, ON_MOB_INTERACT);
        register_event_handler(EventKind::PlayerDismounted, 0, ON_DISMOUNTED);
        register_tick_system(Stage::Mobs, AttachSide::Before, 0, TICK_DRIVE);
        log("initialized: boat placement + boarding + drive");
    }

    fn handle_event(&mut self, handler_id: u32, payload: &mut EventPayload) -> Outcome {
        match (handler_id, &*payload) {
            (ON_ITEM_USE, EventPayload::ItemUsePre { item, target }) => {
                self.on_boat_item_use(*item, *target)
            }
            (
                ON_MOB_INTERACT,
                EventPayload::MobInteract {
                    id, key, player_id, ..
                },
            ) => {
                if key != BOAT_KEY {
                    return Outcome::Continue;
                }
                self.board(*id, *player_id);
                // A click on a boat never falls through to placement — full
                // boats included.
                Outcome::Cancel
            }
            (ON_DISMOUNTED, EventPayload::PlayerDismounted { player_id, mob_id }) => {
                if let Some(boat) = self.boats.get_mut(mob_id) {
                    boat.riders.retain(|p| p != player_id);
                }
                Outcome::Continue
            }
            _ => Outcome::Continue,
        }
    }

    fn tick_system(&mut self, _system_id: u32) {
        self.drive_boats();
    }
}

impl Vehicles {
    /// A use click with the boat item: on a water surface cell with air above,
    /// spend the item and launch a hull facing away from the player.
    fn on_boat_item_use(&mut self, item: ItemId, target: Option<[i32; 3]>) -> Outcome {
        if Some(item) != self.boat_item {
            return Outcome::Continue;
        }
        let (Some(pos), Some(water)) = (target, self.water) else {
            return Outcome::Continue;
        };
        // Stream-final gated reads: `None` means frozen state — treat like a
        // miss and keep the item.
        if get_block(pos) != Some(water)
            || get_block([pos[0], pos[1] + 1, pos[2]]) != Some(BlockId::AIR)
        {
            return Outcome::Continue;
        }
        // Spend first, launch second: a raced-empty hand refuses cleanly, and
        // a failed spawn (mob cap) refunds — the item is never lost.
        if !consume_held(item, 1) {
            return Outcome::Continue;
        }
        let player = player_state();
        // Bow away from the player: mob yaw is π from player yaw.
        let yaw = player.yaw + std::f32::consts::PI;
        let feet = [
            pos[0] as f32 + 0.5,
            pos[1] as f32 + 0.9,
            pos[2] as f32 + 0.5,
        ];
        if !spawn_mob_checked(BOAT_KEY, feet, yaw) {
            give_item(BOAT_KEY, 1);
        }
        Outcome::Cancel
    }

    /// Seat a clicking player in the first free seat and record boarding
    /// order (the first rider steers).
    fn board(&mut self, mob_id: u64, player_id: u8) {
        let Some(seats) = mob_riders(mob_id) else {
            return;
        };
        let Some(seat) = (0..seats.capacity).find(|s| !seats.riders.iter().any(|r| r.seat == *s))
        else {
            return; // full
        };
        if !mob_mount(mob_id, player_id, seat) {
            return; // already mounted / boat gone — the engine said no
        }
        let boat = self.boats.entry(mob_id).or_insert_with(|| {
            // Seed the heading from the live hull so a reloaded (or drifted)
            // boat doesn't snap to a stale angle on first boarding. The
            // clicking player is the acting session, so a small radius around
            // them always covers the boat they just clicked.
            let p = player_state();
            let yaw = mobs_in_radius(p.pos, 8.0)
                .into_iter()
                .find(|m| m.id == mob_id)
                .map(|m| m.yaw)
                .unwrap_or(0.0);
            Boat::new(yaw)
        });
        boat.riders.push(player_id);
    }

    /// One momentum step per live boat: read the driver's input, integrate
    /// speed and yaw rate under drag, issue the drive intent, and animate the
    /// oars on stroke transitions. A hull that stops answering (died,
    /// despawned, unloaded) or has gone still with nobody aboard retires.
    fn drive_boats(&mut self) {
        let mut retired: Vec<u64> = Vec::new();
        for (&id, boat) in self.boats.iter_mut() {
            let input = boat.riders.first().and_then(|&driver| player_input(driver));
            let (forward, strafe) = input.map_or((0.0, 0.0), |i| (i.forward, i.strafe));

            boat.speed = ((boat.speed + forward * ACCEL) * DRAG).clamp(-MAX_REVERSE, MAX_FORWARD);
            // Strafe + turns the rudder right; increasing mob yaw turns left.
            boat.yaw_vel = (boat.yaw_vel - strafe * TURN_ACCEL) * TURN_DRAG;
            boat.yaw = wrap_yaw(boat.yaw + boat.yaw_vel);

            let facing = mob_facing_xz(boat.yaw);
            let vel = [facing[0] * boat.speed, facing[1] * boat.speed];
            if !mob_drive(id, vel, Some(boat.yaw)) {
                retired.push(id); // gone — riders already heard dismounted
                continue;
            }

            // Oar playback follows the INPUT, per stroke side: turning left
            // pulls only the right oar, turning right only the left, forward
            // both, backing water rows in reverse — and a released oar
            // settles gently back onto its rest pose (see `steer_oar`). The
            // animations activate once and stay active; playback is the
            // whole control surface.
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
                if !steer_oar(id, "row_left", left) || !steer_oar(id, "row_right", right) {
                    deactivate_oars(id);
                    boat.oars_active = false;
                }
            }

            if boat.riders.is_empty()
                && boat.speed.abs() < REST_SPEED
                && boat.yaw_vel.abs() < REST_YAW_VEL
            {
                retired.push(id); // at rest, unmanned — nothing left to drive
            }
        }
        for id in retired {
            self.boats.remove(&id);
        }
    }
}

/// Adopt an already-complete pair (including an autonomous settle left by a
/// retired boat), or activate a fresh pair as one parked transaction. A
/// partial pre-existing pair is normalized through the same rollback path.
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

/// Activate both oars and immediately park both at rate 0. No caller can
/// observe a successful half-pair: any failed activation/park rolls back all
/// membership changes.
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

/// Apply input policy against the engine's current layer state. Released oars
/// seek from the authoritative phase to the nearest authored rest cycle;
/// active seeks are left alone to finish autonomously.
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

register_mod!(Vehicles);
