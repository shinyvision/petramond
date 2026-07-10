//! Server-side movement, on the tick: integrate each session from latched
//! intent (F2), then soft-accept a validated client transform claim (F1).
//! See WIKI/client-prediction.md — the claim checks are the anti-cheat
//! surface: velocity envelope, displacement bound, and body penetration.

use crate::mathh::Vec3;
use crate::player::{self, Input};

use super::game::ServerGame;
use crate::game::tick::TICK_DT;

/// Base allowance of the claim-closeness ring, in ticks of worst-case
/// legitimate speed on top of the observed claim gap: absorbs frame/tick
/// phase and entity-push jitter.
const CLAIM_DRIFT_TICKS: f32 = 2.0;
/// Flat allowance on top of the speed-proportional drift bound (step-ups,
/// shoves, float noise).
const CLAIM_DRIFT_SLACK: f32 = 1.0;
/// Cap on the claim gap the drift ring scales with: past this the client
/// must adopt `SelfTransform` corrections instead of stretching the ring
/// (bounds how far withheld updates can displace a player).
const MAX_CLAIM_GAP_TICKS: u32 = 40;
/// Claimed-velocity headroom over the physics caps (quantization, transient
/// pushes). Applied to each axis envelope.
const CLAIM_VEL_SLACK: f32 = 1.25;
/// How deep the claimed body may overlap solid collision geometry before the
/// claim is rejected: shallow contact from step-up easing and float error is
/// legitimate; a body meaningfully inside a block is not.
const PENETRATION_TOL: f32 = 0.1;

impl ServerGame {
    /// Integrate one session's movement on the fixed tick from latched intent
    /// (F2), then soft-accept a validated client claim when it is close (F1).
    pub(crate) fn tick_movement(&mut self, s: usize) {
        let (wishdir, jump, sprint, claimed_pos, claimed_vel, claimed_on_ground, spectator, fresh) = {
            let sess = &self.sessions[s];
            (
                sess.move_wishdir,
                sess.move_jump && sess.intent_gameplay,
                sess.move_sprint && sess.intent_gameplay,
                sess.claim_pos,
                sess.claim_vel,
                sess.claim_on_ground,
                sess.player.is_spectator(),
                sess.claim_fresh,
            )
        };
        // How many ticks the server free-ran since the previous claim — a
        // slow client's report is that much staler, so the closeness ring
        // (and the correction deadband) widen with it instead of
        // rubber-banding every frame gap.
        let gap = {
            let sess = &mut self.sessions[s];
            if fresh {
                std::mem::replace(&mut sess.ticks_since_claim, 0)
            } else {
                sess.ticks_since_claim = sess.ticks_since_claim.saturating_add(1);
                0
            }
        };
        self.sessions[s].claim_fresh = false;

        let input = Input {
            wishdir,
            jump,
            sprint,
        };
        {
            let Self {
                world, sessions, ..
            } = self;
            let sess = &mut sessions[s];
            if spectator || sess.player.columns_loaded(world) {
                sess.player.update(TICK_DT, world, input);
            }
        }

        // F1: only soft-accept a claim from a PlayerUpdate this pump. Stale
        // claims must not yank the player every tick (tests and idle sessions).
        let accept_claim = fresh
            && claim_velocity_plausible(claimed_vel, spectator)
            && (claimed_pos - self.sessions[s].player.pos).length()
                <= claim_drift_ring(spectator, gap)
            && claim_not_deeply_penetrating(claimed_pos, &self.world, spectator);

        let sess = &mut self.sessions[s];
        if accept_claim {
            sess.player.pos = claimed_pos;
            sess.player.vel = claimed_vel;
            sess.player.on_ground = claimed_on_ground;
        }

        let pos = sess.player.pos;
        let on_ground = sess.player.on_ground;

        let in_water = self.world.water_cell_at(
            pos.x.floor() as i32,
            (pos.y + player::WATER_PROBE_Y).floor() as i32,
            pos.z.floor() as i32,
        );
        let sess = &mut self.sessions[s];
        if spectator {
            sess.fall.reset(pos.y);
            sess.pending_fall = 0.0;
            sess.pending_splash = 0.0;
        } else {
            match sess.fall.observe(pos.y, on_ground, in_water) {
                Some(super::player::FallOutcome::Landed(dist)) => {
                    sess.pending_fall = sess.pending_fall.max(dist);
                }
                Some(super::player::FallOutcome::Splashed(dist)) => {
                    sess.pending_splash = sess.pending_splash.max(dist);
                }
                None => {}
            }
        }
        // Do NOT overwrite last_reported_transform here: it stays the client's
        // claim so a rejected claim (or tick teleport) ships SelfTransform.
    }
}

/// The claimed velocity must fit the physics envelope: horizontal speed within
/// the sprint cap, vertical within [terminal fall, jump take-off]. Checked
/// per-axis — a legitimate sprint jump combines BOTH caps, so a single
/// 3D-magnitude test would reject every airborne claim.
fn claim_velocity_plausible(vel: Vec3, spectator: bool) -> bool {
    if !vel.is_finite() {
        return false;
    }
    if spectator {
        return vel.length() <= player::SPECTATOR_SPRINT * CLAIM_VEL_SLACK;
    }
    let horizontal = Vec3::new(vel.x, 0.0, vel.z).length();
    horizontal <= player::SPRINT * 1.5 * CLAIM_VEL_SLACK
        && vel.y <= player::JUMP_V0 * CLAIM_VEL_SLACK
        && vel.y >= -player::TERMINAL * CLAIM_VEL_SLACK
}

/// How far a claim may sit from the server's own integration and still be
/// soft-accepted — the anti-teleport bound. Speed-proportional so spectators
/// (who legitimately fly fast) get a wider ring, and scaled by `gap_ticks`
/// (ticks since the previous claim): a slow client's report is stale by the
/// whole gap and both integrations legitimately drifted apart over it. The
/// displacement RATE stays capped at legitimate speed either way.
pub(crate) fn claim_drift_ring(spectator: bool, gap_ticks: u32) -> f32 {
    let max_speed = if spectator {
        player::SPECTATOR_SPRINT * CLAIM_VEL_SLACK
    } else {
        player::TERMINAL * CLAIM_VEL_SLACK
    };
    let ticks = gap_ticks.min(MAX_CLAIM_GAP_TICKS) as f32 + CLAIM_DRIFT_TICKS;
    max_speed * TICK_DT * ticks + CLAIM_DRIFT_SLACK
}

/// Velocity divergence beyond which a `SelfTransform` correction ships:
/// large enough to ignore the gravity the server accrued past the client's
/// last report (scaled by the claim gap), small enough that a knockback
/// impulse corrects immediately.
pub(crate) fn vel_correction_eps(gap_ticks: u32) -> f32 {
    4.0 + gap_ticks.min(MAX_CLAIM_GAP_TICKS) as f32 * player::GRAVITY * TICK_DT
}

/// Whether the claimed body position overlaps solid collision geometry deeper
/// than [`PENETRATION_TOL`] — the anti-noclip check, over every cell the
/// player AABB spans.
fn claim_not_deeply_penetrating(pos: Vec3, world: &crate::world::World, spectator: bool) -> bool {
    if spectator {
        return true;
    }
    let min = Vec3::new(
        pos.x - player::HALF_W + PENETRATION_TOL,
        pos.y + PENETRATION_TOL,
        pos.z - player::HALF_W + PENETRATION_TOL,
    );
    let max = Vec3::new(
        pos.x + player::HALF_W - PENETRATION_TOL,
        pos.y + player::HEIGHT - PENETRATION_TOL,
        pos.z + player::HALF_W - PENETRATION_TOL,
    );
    for x in (min.x.floor() as i32)..=(max.x.floor() as i32) {
        for y in (min.y.floor() as i32)..=(max.y.floor() as i32) {
            for z in (min.z.floor() as i32)..=(max.z.floor() as i32) {
                for b in world.collision_boxes_at(x, y, z) {
                    let bmin = Vec3::new(
                        x as f32 + b.min[0],
                        y as f32 + b.min[1],
                        z as f32 + b.min[2],
                    );
                    let bmax = Vec3::new(
                        x as f32 + b.max[0],
                        y as f32 + b.max[1],
                        z as f32 + b.max[2],
                    );
                    if min.x < bmax.x
                        && max.x > bmin.x
                        && min.y < bmax.y
                        && max.y > bmin.y
                        && min.z < bmax.z
                        && max.z > bmin.z
                    {
                        return false;
                    }
                }
            }
        }
    }
    true
}
