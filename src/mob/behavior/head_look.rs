//! Head-look: where an idle mob points its head.
//!
//! While the mob is idle, every so often it re-decides what to look at: if the player
//! is within [`LOOK_RADIUS`] it's very likely to lock onto them, otherwise it picks a
//! small random glance. The look-at-player target is recomputed each tick so the head
//! tracks a moving player smoothly (the instance eases toward it slowly). Yaw is taken
//! relative to the body; pitch comes from the height difference between the mob's head
//! and the player — level → look straight, player taller/shorter → look up/down.
//!
//! The head can only crane so far ([`MAX_HEAD_YAW`] = 90° either way): if the player is
//! beyond that arc — they've walked round behind the mob — it simply can't be looked at,
//! so the mob gives up and the head recentres forward, rather than wrenching the head
//! around (or snapping it through the back as the player crosses directly behind). While
//! the mob is navigating, this yields too, so the instance recentres the head.
//!
//! The renderer applies the result to the model's `head` bone, and suppresses it while
//! an animation is already moving the head — so a model with no `head` bone, or one
//! whose active animation drives the head, simply ignores this.

use crate::mathh::Vec3;

use super::super::brain::{AiBehavior, AiCtx, BehaviorOutput, HeadLook};

/// Player within this distance (m) of the mob's head → likely to be looked at.
const LOOK_RADIUS: f32 = 2.0;
/// Chance, on each re-decision while the player is near, to lock onto them.
const LOOK_AT_PLAYER_CHANCE: f32 = 0.85;
/// Head rotation limits relative to the body (radians): 90° yaw either way (a real
/// neck can't crane past square-sideways, and capping here stops the head spinning all
/// the way round as the player circles the mob), ~50° pitch.
const MAX_HEAD_YAW: f32 = std::f32::consts::FRAC_PI_2;
const MAX_HEAD_PITCH: f32 = 0.9;
/// Range of a random idle glance (radians).
const GLANCE_YAW: f32 = 1.1;
const GLANCE_PITCH: f32 = 0.25;
/// Ticks between head re-decisions (~1–3 s at 20 TPS) — slow, deliberate glances.
const REPICK_MIN_TICKS: u32 = 20;
const REPICK_SPAN_TICKS: u32 = 40;

enum LookMode {
    /// Track the player (recomputed each tick so the head follows them).
    AtPlayer,
    /// Hold a fixed random glance.
    Glance(HeadLook),
}

pub struct HeadLookAi {
    mode: LookMode,
    /// Ticks until the next re-decision.
    timer: u32,
}

impl HeadLookAi {
    pub fn new() -> Self {
        HeadLookAi {
            mode: LookMode::Glance(HeadLook {
                yaw: 0.0,
                pitch: 0.0,
            }),
            timer: 0,
        }
    }
}

impl AiBehavior for HeadLookAi {
    fn tick(&mut self, ctx: &mut AiCtx) -> BehaviorOutput {
        // Navigating: no head-look, so the instance recentres the head forward.
        if !ctx.nav_idle {
            self.timer = 0;
            return BehaviorOutput::default();
        }
        // Idle: occasionally re-decide what to look at.
        if self.timer == 0 {
            self.mode = pick_mode(ctx);
            self.timer = REPICK_MIN_TICKS + (ctx.rng.next_f32() * REPICK_SPAN_TICKS as f32) as u32;
        } else {
            self.timer -= 1;
        }
        // A glance is always a valid (in-range) angle; looking at the player may not be
        // (they could be behind the mob), in which case `look_at_player` yields `None`
        // and the head recentres forward — the mob can't crane round to see them.
        let target = match self.mode {
            LookMode::AtPlayer => look_at_player(ctx),
            LookMode::Glance(h) => Some(h),
        };
        BehaviorOutput {
            head_look: target,
            ..Default::default()
        }
    }
}

/// Decide whether to track the player (likely when near) or take a random glance.
fn pick_mode(ctx: &mut AiCtx) -> LookMode {
    let to_player = ctx.player_pos - head_pos(ctx);
    let near = to_player.length_squared() <= LOOK_RADIUS * LOOK_RADIUS;
    if near && ctx.rng.next_f32() < LOOK_AT_PLAYER_CHANCE {
        LookMode::AtPlayer
    } else {
        LookMode::Glance(HeadLook {
            yaw: ctx.rng.next_signed() * GLANCE_YAW,
            pitch: ctx.rng.next_signed() * GLANCE_PITCH,
        })
    }
}

/// Head orientation (relative to the body) that points at the player, or `None` if the
/// player is outside the head's turn arc (so the mob can't look at them — see
/// [`head_look_toward`]).
fn look_at_player(ctx: &AiCtx) -> Option<HeadLook> {
    head_look_toward(ctx.player_pos - head_pos(ctx), ctx.yaw)
}

/// Head orientation (relative to the body) to look along `to` (the head→target vector
/// in world space, with body facing `body_yaw`), or `None` when the target lies beyond
/// the head's ±[`MAX_HEAD_YAW`] turn arc — past that the mob physically can't face it, so
/// it shouldn't try (the caller recentres the head forward instead). Yaw is the
/// horizontal bearing minus the body yaw; pitch comes from the height difference and is
/// clamped to the head's tilt limit.
fn head_look_toward(to: Vec3, body_yaw: f32) -> Option<HeadLook> {
    let horiz = (to.x * to.x + to.z * to.z).sqrt();
    // World bearing to the target using the model's `-Z`-forward convention.
    let world_yaw = (-to.x).atan2(-to.z);
    let yaw = wrap_angle(world_yaw - body_yaw);
    // Beyond the neck's reach → can't be looked at. Don't clamp to the limit (that would
    // leave the head straining sideways at a target it isn't facing, and snap it across
    // the back as the target crosses behind); give up so the head returns forward.
    if yaw.abs() > MAX_HEAD_YAW {
        return None;
    }
    // Positive pitch looks up (target above the mob's head); clamp the tilt.
    let pitch =
        to.y.atan2(horiz.max(1e-3))
            .clamp(-MAX_HEAD_PITCH, MAX_HEAD_PITCH);
    Some(HeadLook { yaw, pitch })
}

/// Approximate world position of the mob's head (feet + head height).
fn head_pos(ctx: &AiCtx) -> Vec3 {
    Vec3::new(ctx.pos.x, ctx.pos.y + ctx.head_height, ctx.pos.z)
}

fn wrap_angle(a: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    (a + PI).rem_euclid(TAU) - PI
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_2;

    #[test]
    fn looks_straight_at_a_target_dead_ahead() {
        // Model faces -Z; a target straight in front needs ~no head turn.
        let h = head_look_toward(Vec3::new(0.0, 0.0, -2.0), 0.0).expect("in front is lookable");
        assert!(h.yaw.abs() < 1e-3, "no yaw needed dead ahead: {}", h.yaw);
        assert!(
            h.pitch.abs() < 1e-3,
            "level target needs no pitch: {}",
            h.pitch
        );
    }

    #[test]
    fn square_to_the_side_is_the_furthest_it_will_look() {
        // A target exactly 90° to the side is right at the limit — still lookable.
        let h = head_look_toward(Vec3::new(2.0, 0.0, 0.0), 0.0)
            .expect("square sideways is at the limit");
        assert!(
            (h.yaw.abs() - FRAC_PI_2).abs() < 1e-4,
            "looks square sideways: {}",
            h.yaw
        );
    }

    #[test]
    fn gives_up_when_the_target_is_past_ninety_degrees() {
        // Just past square — and directly behind — the mob can't crane round, so it
        // stops looking (None) instead of clamping/whipping the head around.
        assert!(
            head_look_toward(Vec3::new(2.0, 0.0, 0.5), 0.0).is_none(),
            "a target behind the shoulder is given up on"
        );
        assert!(
            head_look_toward(Vec3::new(0.0, 0.0, 2.0), 0.0).is_none(),
            "a target directly behind is given up on"
        );
    }

    #[test]
    fn the_turn_arc_is_measured_from_the_body_facing() {
        // The same world target is in range when the body faces it and out of range once
        // the body has turned far enough away — the cap is relative to the body, not world.
        let target = Vec3::new(0.0, 0.0, -2.0); // due -Z in world
        assert!(
            head_look_toward(target, 0.0).is_some(),
            "in front of a -Z-facing body"
        );
        assert!(
            head_look_toward(target, FRAC_PI_2 + 0.1).is_none(),
            "body turned >90° away can't look back at it"
        );
    }

    #[test]
    fn pitch_is_clamped_to_the_tilt_limit() {
        // A target sharply above (within yaw range) tilts the head up, but no further
        // than the pitch limit.
        let h = head_look_toward(Vec3::new(0.0, 5.0, -0.1), 0.0).expect("in front is lookable");
        assert!(h.pitch > 0.0, "looks up at a higher target: {}", h.pitch);
        assert!(
            h.pitch <= MAX_HEAD_PITCH + 1e-6,
            "pitch is capped: {}",
            h.pitch
        );
    }
}
