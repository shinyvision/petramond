//! Server-side movement, on the tick: integrate each session from latched
//! intent (F2), then soft-accept a validated client transform claim (F1).
//! See WIKI/client-prediction.md — the claim checks are the anti-cheat
//! surface: velocity envelope, per-axis displacement bound, body
//! penetration, and ground-support verification for the fall tracker. The
//! same drift ring bounds the reach eye ([`reach_eye`]) so a fabricated
//! claim cannot grant remote block interaction.

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
/// Horizontal speed cap shared by the velocity envelope and the horizontal
/// drift ring: sprint plus headroom for every legitimate horizontal transient
/// (PvP knockback 5.0, mob-strike knockback 6.5, entity push). Sharing the cap
/// keeps the two checks consistent — no claim that passes the velocity
/// envelope is rejected by the ring at the same speed.
const CLAIM_H_SPEED: f32 = player::SPRINT * 1.5;
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
        // Where the server's OWN integration ended this tick, if grounded —
        // trusted ground contact for the fall tracker below (claim adoption
        // overwrites the transform before the tracker samples it).
        let integrated_ground_y = {
            let Self {
                world, sessions, ..
            } = self;
            let sess = &mut sessions[s];
            if spectator || sess.player.columns_loaded(world) {
                sess.player.update(TICK_DT, world, input);
                (!spectator && sess.player.on_ground).then(|| sess.player.pos.y)
            } else {
                None
            }
        };

        // F1: only soft-accept a claim from a PlayerUpdate this pump. Stale
        // claims must not yank the player every tick (tests and idle sessions).
        let accept_claim = fresh
            && claim_velocity_plausible(claimed_vel, spectator)
            && claim_within_drift(spectator, gap, claimed_pos - self.sessions[s].player.pos)
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
        // The fall tracker must not trust a CLAIMED on_ground flag: faking
        // "grounded" every tick mid-fall would reset the peak and evade the
        // landing. When the flag came from an accepted claim, verify it
        // against real support under the feet — a legit grounded claim always
        // has geometry there, so nothing tightens for real clients. The
        // server's own integration (rejected claim) is already trustworthy,
        // and unloaded columns can't answer, so both keep the flag as-is.
        let grounded_for_fall = on_ground
            && (!accept_claim
                || !self.sessions[s].player.columns_loaded(&self.world)
                || feet_supported(pos, &self.world));
        let sess = &mut self.sessions[s];
        if spectator {
            sess.fall.reset(pos.y);
            sess.pending_fall = 0.0;
            sess.pending_splash = 0.0;
        } else {
            // Sprinting down stairs touches each step for only a frame or
            // two, so the once-per-tick claim samples are legitimately
            // airborne for the whole descent and the tracker would measure
            // the staircase as one tall fall. The server's own integration
            // (trusted physics, never a client flag) did land on those
            // steps: when the claim sample is airborne and dry, re-anchor
            // the tracker at the integration's contact first — which also
            // latches any real landing that happened between claim samples.
            if !grounded_for_fall && !in_water {
                if let Some(y) = integrated_ground_y {
                    if let Some(super::player::FallOutcome::Landed(dist)) =
                        sess.fall.observe(y, true, false)
                    {
                        sess.pending_fall = sess.pending_fall.max(dist);
                    }
                }
            }
            match sess.fall.observe(pos.y, grounded_for_fall, in_water) {
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
    horizontal <= CLAIM_H_SPEED * CLAIM_VEL_SLACK
        && vel.y <= player::JUMP_V0 * CLAIM_VEL_SLACK
        && vel.y >= -player::TERMINAL * CLAIM_VEL_SLACK
}

/// One axis of the anti-teleport bound: how far a claim may sit from the
/// server's own integration at `rate` and still be soft-accepted. Scaled by
/// `gap_ticks` (ticks since the previous claim): a slow client's report is
/// stale by the whole gap and both integrations legitimately drifted apart
/// over it. The displacement RATE stays capped at legitimate speed either way.
fn drift_ring(rate: f32, gap_ticks: u32) -> f32 {
    let ticks = gap_ticks.min(MAX_CLAIM_GAP_TICKS) as f32 + CLAIM_DRIFT_TICKS;
    rate * TICK_DT * ticks + CLAIM_DRIFT_SLACK
}

/// Whether a claimed position sits within the drift ring of the server's own
/// integration. Per-axis for survival players — the horizontal ring runs at
/// the horizontal velocity envelope's speed, the vertical at terminal fall —
/// so a fall stays as relaxed as ever while a fabricated sideways jump can no
/// longer ride the (much larger) terminal-speed allowance. Spectators
/// legitimately fly fast in any direction and keep one isotropic ring.
pub(crate) fn claim_within_drift(spectator: bool, gap_ticks: u32, delta: Vec3) -> bool {
    if spectator {
        return delta.length() <= drift_ring(player::SPECTATOR_SPRINT * CLAIM_VEL_SLACK, gap_ticks);
    }
    let horizontal = Vec3::new(delta.x, 0.0, delta.z).length();
    horizontal <= drift_ring(CLAIM_H_SPEED * CLAIM_VEL_SLACK, gap_ticks)
        && delta.y.abs() <= drift_ring(player::TERMINAL * CLAIM_VEL_SLACK, gap_ticks)
}

/// The eye every block-reach check for this session measures from: the
/// CLAIMED eye while the claim sits inside the F1 drift ring of the server's
/// own integration, else the integrated eye. A legitimate client's claim is
/// always inside the ring (outside it the claim is also rejected for movement
/// and a `SelfTransform` correction is in flight), so reach never tightens
/// for real clients — but a fabricated far-away claim no longer grants
/// remote reach over mining, placement, and interaction.
pub(crate) fn reach_eye(sess: &crate::server::player::ConnectedPlayer) -> Vec3 {
    let delta = sess.claim_pos - sess.player.pos;
    let base = if claim_within_drift(sess.player.is_spectator(), sess.ticks_since_claim, delta) {
        sess.claim_pos
    } else {
        sess.player.pos
    };
    base + Vec3::new(0.0, player::EYE, 0.0)
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
    !aabb_hits_collision(world, min, max)
}

/// How far below the feet the ground-support probe reaches. Generous: any
/// legitimately grounded pose has a collision-box top well inside this band
/// (float noise and step-up easing keep feet within millimetres of the top).
const GROUND_PROBE_DEPTH: f32 = 0.25;
const GROUND_PROBE_UP: f32 = 0.05;

/// Whether solid collision geometry sits directly under the feet at `pos` —
/// the verification behind an accepted claim's `on_ground` flag (fall
/// measurement only; the flag itself is still adopted for physics).
fn feet_supported(pos: Vec3, world: &crate::world::World) -> bool {
    let min = Vec3::new(
        pos.x - player::HALF_W,
        pos.y - GROUND_PROBE_DEPTH,
        pos.z - player::HALF_W,
    );
    let max = Vec3::new(
        pos.x + player::HALF_W,
        pos.y + GROUND_PROBE_UP,
        pos.z + player::HALF_W,
    );
    aabb_hits_collision(world, min, max)
}

/// Whether the world AABB `[min, max]` overlaps any collision box of any cell
/// it spans.
fn aabb_hits_collision(world: &crate::world::World, min: Vec3, max: Vec3) -> bool {
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
                        return true;
                    }
                }
            }
        }
    }
    false
}
