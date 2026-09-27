use crate::player::{self, Input};
use petramond_math::math::Vec3;

use super::game::ServerGame;
use super::player::ConnectedPlayer;
use super::sessions::SessionRegistry;
use crate::events::tick::TICK_DT;
use crate::world::ServerWorld;

const CLAIM_DRIFT_TICKS: f32 = 2.0;
const CLAIM_DRIFT_SLACK: f32 = 1.0;
const MAX_CLAIM_GAP_TICKS: u32 = 40;
const CLAIM_VEL_SLACK: f32 = crate::entity::VELOCITY_SLACK;
const CLAIM_H_SPEED: f32 = player::SPRINT * 1.5;
const PENETRATION_TOL: f32 = 0.1;

pub(in crate::server) fn tick_movements(world: &ServerWorld, sessions: &mut SessionRegistry) {
    let obstacles = world.mobs().solid_obstacles();
    for sess in sessions {
        integrate_session(world, sess, &obstacles);
    }
}

impl ServerGame {
    #[cfg(any(test, feature = "test-support"))]
    pub fn tick_movement(&mut self, s: usize) {
        let obstacles = self.world.mobs().solid_obstacles();
        integrate_session(&self.world, &mut self.sessions[s], &obstacles);
    }
}

/// Integrate one session's movement on the fixed tick from latched intent
/// (F2), then soft-accept a validated client claim when it is close (F1).
///
/// A MOUNTED session skips all of it: the riding pass owns the transform
/// (the player is slaved to its seat after the mobs move), claims are
/// neither integrated nor adopted (the client slaves itself to the same
/// replicated mount), and no fall accrues. Claim staleness bookkeeping
/// still runs so the drift ring is honest on the dismount tick.
fn integrate_session(
    world: &ServerWorld,
    sess: &mut ConnectedPlayer,
    obstacles: &[petramond_world::collision::DynBox],
) {
    let wishdir = sess.input.move_wishdir;
    let jump = sess.input.move_jump && sess.input.intent_gameplay;
    let sprint = sess.input.move_sprint && sess.input.intent_gameplay;
    let sneak = sess.sneaking();
    let claimed_pos = sess.input.claim_pos;
    let claimed_vel = sess.input.claim_vel;
    let claimed_on_ground = sess.input.claim_on_ground;
    let spectator = sess.player.is_spectator();
    let fresh = sess.input.claim_fresh;
    let gap = if fresh {
        std::mem::replace(&mut sess.input.ticks_since_claim, 0)
    } else {
        sess.input.ticks_since_claim = sess.input.ticks_since_claim.saturating_add(1);
        0
    };
    sess.input.claim_fresh = false;
    if sess.sim.mount.is_some() {
        return;
    }

    let input = Input {
        wishdir,
        jump,
        sprint,
        sneak,
    };
    let integrated_ground_y = if spectator
        || (sess.player.columns_loaded(world.data()) && body_terrain_final(&sess.player, world))
    {
        sess.player
            .update_with_obstacles(TICK_DT, world.data(), input, obstacles);
        (!spectator && sess.player.on_ground).then_some(sess.player.pos.y)
    } else {
        None
    };

    let fly_scale = sess.player.fly_scale();
    let velocity_plausible = if sess.player.is_flying() {
        claimed_vel.is_finite()
            && claimed_vel.length()
                <= crate::player::creative_flight_speed(true) * fly_scale * CLAIM_VEL_SLACK
    } else {
        claim_velocity_plausible(claimed_vel, spectator, fly_scale)
    };
    let accept_claim = fresh
        && velocity_plausible
        && claim_within_drift(spectator, gap, claimed_pos - sess.player.pos)
        && (claim_not_deeply_penetrating(claimed_pos, world, obstacles, spectator)
            || !claim_not_deeply_penetrating(sess.player.pos, world, obstacles, spectator));

    if accept_claim {
        sess.player.pos = claimed_pos;
        sess.player.vel = claimed_vel;
        sess.player.on_ground = claimed_on_ground;
    }

    let pos = sess.player.pos;
    let on_ground = sess.player.on_ground;

    let immersion =
        world
            .data()
            .body_fluid(pos, player::HEIGHT, petramond_world::fluid::Buoyancy::Swim);
    let swimming = immersion.is_some();
    let climbing = !swimming
        && world
            .data()
            .climb_at(
                pos.x.floor() as i32,
                pos.y.floor() as i32,
                pos.z.floor() as i32,
            )
            .is_some();
    // The fall tracker must not trust a CLAIMED on_ground flag: faking
    // "grounded" every tick mid-fall would reset the peak and evade the
    // landing. When the flag came from an accepted claim, verify it against
    // real support under the feet — a legit grounded claim always has
    // geometry there, so nothing tightens for real clients. The server's own
    // integration (rejected claim) is already trustworthy, and unloaded
    // columns can't answer, so both keep the flag as-is.
    let grounded_for_fall = on_ground
        && (!accept_claim
            || !sess.player.columns_loaded(world.data())
            || feet_supported(pos, world, obstacles));
    if sess.player.is_invulnerable() {
        sess.sim.fall.reset(pos.y);
        sess.sim.pending_fall = 0.0;
        sess.sim.pending_splash = 0.0;
    } else if climbing {
        sess.sim.fall.reset(pos.y);
    } else {
        // Sprinting downstairs, each step only gets touched for a frame or two, so the
        // once-per-tick claim samples look airborne the whole way down and the tracker would count
        // the staircase as one tall fall. Server integration (trusted physics, never a client
        // flag) did land on those steps, so when a claim sample is airborne and dry, re-anchor the
        // tracker at the integration's contact first. That also catches any real landing between
        // claim samples.
        if !grounded_for_fall && !swimming {
            if let Some(y) = integrated_ground_y {
                if let Some(super::player::FallOutcome::Landed(dist)) =
                    sess.sim.fall.observe(y, true, false)
                {
                    sess.sim.pending_fall = sess.sim.pending_fall.max(dist);
                }
            }
        }
        match sess.sim.fall.observe(pos.y, grounded_for_fall, swimming) {
            Some(super::player::FallOutcome::Landed(dist)) => {
                sess.sim.pending_fall = sess.sim.pending_fall.max(dist);
            }
            Some(super::player::FallOutcome::Splashed(dist))
                if immersion.is_some_and(|sample| sample.fluid.splash.is_some()) =>
            {
                sess.sim.pending_splash = sess.sim.pending_splash.max(dist);
            }
            _ => {}
        }
    }
}

fn claim_velocity_plausible(vel: Vec3, spectator: bool, fly_scale: f32) -> bool {
    if !vel.is_finite() {
        return false;
    }
    if spectator {
        return vel.length() <= player::SPECTATOR_SPRINT * fly_scale * CLAIM_VEL_SLACK;
    }
    let horizontal = Vec3::new(vel.x, 0.0, vel.z).length();
    horizontal <= CLAIM_H_SPEED * CLAIM_VEL_SLACK
        && vel.y <= player::JUMP_V0 * CLAIM_VEL_SLACK
        && vel.y >= -player::TERMINAL * CLAIM_VEL_SLACK
}

fn drift_ring(rate: f32, gap_ticks: u32) -> f32 {
    let ticks = gap_ticks.min(MAX_CLAIM_GAP_TICKS) as f32 + CLAIM_DRIFT_TICKS;
    rate * TICK_DT * ticks + CLAIM_DRIFT_SLACK
}

pub fn claim_within_drift(spectator: bool, gap_ticks: u32, delta: Vec3) -> bool {
    if spectator {
        return delta.length() <= drift_ring(player::SPECTATOR_SPRINT * CLAIM_VEL_SLACK, gap_ticks);
    }
    let horizontal = Vec3::new(delta.x, 0.0, delta.z).length();
    horizontal <= drift_ring(CLAIM_H_SPEED * CLAIM_VEL_SLACK, gap_ticks)
        && delta.y.abs() <= drift_ring(player::TERMINAL * CLAIM_VEL_SLACK, gap_ticks)
}

fn body_terrain_final(player: &crate::player::Player, world: &crate::world::ServerWorld) -> bool {
    let (hw, height) = (f64::from(player::HALF_W), f64::from(player::HEIGHT));
    let (x0, x1) = (
        (player.pos.x - hw).floor() as i32,
        (player.pos.x + hw).floor() as i32,
    );
    let (z0, z1) = (
        (player.pos.z - hw).floor() as i32,
        (player.pos.z + hw).floor() as i32,
    );
    let (y0, y1) = (
        (player.pos.y - 1.0).floor() as i32,
        (player.pos.y + height).floor() as i32,
    );
    for y in y0..=y1 {
        for z in z0..=z1 {
            for x in x0..=x1 {
                if !world.physics_cell_final_at(x, y, z) {
                    return false;
                }
            }
        }
    }
    true
}

/// Reach checks measure from the claimed eye if it's inside the F1 drift ring, else from the
/// integrated eye. Honest clients always claim inside the ring (outside it, movement rejects the
/// claim too and a `SelfTransform` fix is on its way), so they never lose reach. Faking a far-away
/// eye doesn't buy remote mining, placing or interacting.
pub fn reach_eye(sess: &ConnectedPlayer) -> petramond_math::world_pos::WorldPos {
    let delta = sess.input.claim_pos - sess.player.pos;
    let spectator = sess.player.is_spectator();
    let base = if claim_within_drift(spectator, sess.input.ticks_since_claim, delta) {
        sess.input.claim_pos
    } else {
        sess.player.pos
    };
    base + Vec3::new(0.0, player::EYE, 0.0)
}

pub fn vel_correction_eps(gap_ticks: u32) -> f32 {
    4.0 + gap_ticks.min(MAX_CLAIM_GAP_TICKS) as f32 * player::GRAVITY * TICK_DT
}

fn claim_not_deeply_penetrating(
    pos: petramond_math::world_pos::WorldPos,
    world: &crate::world::ServerWorld,
    obstacles: &[petramond_world::collision::DynBox],
    spectator: bool,
) -> bool {
    if spectator {
        return true;
    }
    let (hw, height) = (f64::from(player::HALF_W), f64::from(player::HEIGHT));
    let tol = f64::from(PENETRATION_TOL);
    let min = [pos.x - hw + tol, pos.y + tol, pos.z - hw + tol];
    let max = [pos.x + hw - tol, pos.y + height - tol, pos.z + hw - tol];
    !aabb_hits_collision(world, min, max)
        && !petramond_world::collision::aabb_hits_dynamic(
            min,
            max,
            obstacles,
            petramond_world::collision::NOT_AN_ENTITY,
        )
}

const GROUND_PROBE_DEPTH: f32 = 0.25;
const GROUND_PROBE_UP: f32 = 0.05;

fn feet_supported(
    pos: petramond_math::world_pos::WorldPos,
    world: &crate::world::ServerWorld,
    obstacles: &[petramond_world::collision::DynBox],
) -> bool {
    let hw = f64::from(player::HALF_W);
    let min = [
        pos.x - hw,
        pos.y - f64::from(GROUND_PROBE_DEPTH),
        pos.z - hw,
    ];
    let max = [pos.x + hw, pos.y + f64::from(GROUND_PROBE_UP), pos.z + hw];
    aabb_hits_collision(world, min, max)
        || petramond_world::collision::aabb_hits_dynamic(
            min,
            max,
            obstacles,
            petramond_world::collision::NOT_AN_ENTITY,
        )
}

pub fn aabb_hits_collision(
    world: &crate::world::ServerWorld,
    min: [f64; 3],
    max: [f64; 3],
) -> bool {
    petramond_world::collision::aabb_hits_cells(min, max, |x, y, z| {
        world.data().collision_boxes_at(x, y, z)
    })
}
