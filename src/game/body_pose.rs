//! The player-body presentation pose shared by the LOCAL third-person view and
//! every REMOTE player: body-yaw follow (head turns freely inside a limit, the
//! body is dragged past it and re-aligns while walking) and the walk-cycle
//! phase/blend. ONE implementation — `game/third_person.rs` drives it from the
//! predicted player, `game/remote_players.rs` from interpolated replicated
//! rows. Everything here is per-frame presentation; nothing feeds the sim.

/// How far the head may yaw away from the body before the body is dragged
/// along (radians, ~45°).
const HEAD_YAW_LIMIT: f32 = std::f32::consts::FRAC_PI_4;
/// Exponential rate at which the body re-aligns to the look direction while
/// walking (a walking body faces where it goes).
const BODY_ALIGN_RATE: f32 = 8.0;
/// Walk-cycle phase advance per block walked (cycles/block): ties the authored
/// 1 s `walk` loop to actual ground speed so sprinting swings faster.
const WALK_CYCLES_PER_BLOCK: f32 = 0.35;
/// Horizontal speed (blocks/s) below which the player counts as standing.
const MOVING_SPEED_SQ: f32 = 0.05 * 0.05;
/// Exponential rate the walk↔stand pose blend settles at, so stopping eases the
/// limbs back to rest instead of snapping to the rest pose in one frame.
const WALK_BLEND_RATE: f32 = 10.0;
/// Exponential rate the stand↔sneak stance blend settles at — the same feel as
/// the walk blend, so crouching down and rising ease identically.
const SNEAK_BLEND_RATE: f32 = 10.0;

/// A player body's presentation pose, advanced once per frame.
#[derive(Copy, Clone, Debug, Default)]
pub(crate) struct BodyPose {
    /// The body's facing yaw (engine yaw space, like `Player::yaw`). Trails
    /// the head within [`HEAD_YAW_LIMIT`]; re-aligns while walking.
    pub(crate) body_yaw: f32,
    /// Seconds into the walk animation while `moving`.
    pub(crate) anim_time: f32,
    pub(crate) moving: bool,
    /// Walk-pose blend weight (`0` standing … `1` full walk cycle), eased
    /// toward `moving` so starts and stops transition instead of snapping.
    pub(crate) walk_weight: f32,
    /// Sneak-stance blend weight (`0` upright … `1` fully crouched), eased
    /// toward the sneak intent. The renderer cross-fades the sneak animation
    /// in by this: its FRAME 0 is the standing-still stance, and while moving
    /// the same clip's cycle replaces the walk cycle.
    pub(crate) sneak_weight: f32,
}

impl BodyPose {
    /// Snap the pose to face `yaw` at rest — entering third person, a remote
    /// player appearing, or a replicated teleport (`snap`), so the model never
    /// pops in mid-turn or spins across a jump.
    pub(crate) fn reset_facing(&mut self, yaw: f32) {
        self.body_yaw = yaw;
        self.anim_time = 0.0;
        self.moving = false;
        self.walk_weight = 0.0;
        self.sneak_weight = 0.0;
    }

    /// Freeze into the lying pose: the body faces `body_yaw` (the bed's
    /// base→pillow yaw) with the walk cycle fully rested.
    pub(crate) fn lie(&mut self, body_yaw: f32) {
        self.body_yaw = body_yaw;
        self.moving = false;
        self.walk_weight = 0.0;
        self.sneak_weight = 0.0;
    }

    /// One frame of the pose: ease the walk blend toward the moving state,
    /// advance the phase by ground speed, and follow the body yaw behind the
    /// head (`head_yaw` = the look yaw). `can_move` gates the walk animation
    /// (false for spectators — the local body never draws for one, but the
    /// gate keeps both drivers identical). `sneaking` eases the sneak-stance
    /// blend in/out.
    pub(crate) fn advance(
        &mut self,
        dt: f32,
        hspeed: f32,
        head_yaw: f32,
        can_move: bool,
        sneaking: bool,
    ) {
        self.moving = hspeed * hspeed > MOVING_SPEED_SQ && can_move;
        // Stand↔sneak blends like walk↔stand: eased, with a snap-to-rest floor
        // so the weight actually reaches 0/1.
        let sneak_target = if sneaking && can_move { 1.0 } else { 0.0 };
        let sneak_settle = 1.0 - (-SNEAK_BLEND_RATE * dt.max(0.0)).exp();
        self.sneak_weight += (sneak_target - self.sneak_weight) * sneak_settle;
        if self.sneak_weight < 0.01 && sneak_target == 0.0 {
            self.sneak_weight = 0.0;
        }
        // Walk↔stand blends instead of snapping: the weight eases toward the
        // moving state; the phase advances only while moving (a stopping body
        // fades its frozen mid-stride pose back to rest).
        let target = if self.moving { 1.0 } else { 0.0 };
        let settle = 1.0 - (-WALK_BLEND_RATE * dt.max(0.0)).exp();
        self.walk_weight += (target - self.walk_weight) * settle;
        if self.moving {
            // Start each fresh stride at phase 0 (but never mid-blend, so a
            // quick stop-start doesn't pop the legs).
            if self.walk_weight < 0.05 {
                self.anim_time = 0.0;
            }
            self.anim_time += dt * hspeed * WALK_CYCLES_PER_BLOCK;
        } else if self.walk_weight < 0.01 {
            self.walk_weight = 0.0;
        }
        self.body_yaw = follow_body_yaw(self.body_yaw, head_yaw, self.moving, dt);
    }
}

/// One frame of the body-yaw follow rule: the head (look) turns freely within
/// [`HEAD_YAW_LIMIT`] of the body; past it the body is dragged along so the
/// neck never over-twists, and while walking the body eases toward the look
/// direction (a walking body faces where it goes).
pub(crate) fn follow_body_yaw(body_yaw: f32, head_yaw: f32, moving: bool, dt: f32) -> f32 {
    let mut body = body_yaw;
    if moving {
        let settle = 1.0 - (-BODY_ALIGN_RATE * dt.max(0.0)).exp();
        body += wrap_angle(head_yaw - body) * settle;
    }
    let diff = wrap_angle(head_yaw - body);
    let clamped = diff.clamp(-HEAD_YAW_LIMIT, HEAD_YAW_LIMIT);
    // Re-derive from the head so the stored yaw stays numerically near it
    // instead of accumulating whole turns.
    head_yaw - clamped
}

/// Wrap an angle difference into `(-π, π]`.
pub(crate) fn wrap_angle(a: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let mut d = a % TAU;
    if d > PI {
        d -= TAU;
    } else if d < -PI {
        d += TAU;
    }
    d
}

/// Interpolate from angle `a` to `b` along the shortest arc (radians) — the
/// shared angular sibling of `Vec3::lerp`, used by the mob scene bake and the
/// remote-player interpolation.
pub(crate) fn lerp_angle(a: f32, b: f32, t: f32) -> f32 {
    a + wrap_angle(b - a) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_look_turns_move_only_the_head() {
        // Idle, head 30° off the body: under the 45° threshold the body stays put.
        let body = follow_body_yaw(0.0, 30f32.to_radians(), false, 0.016);
        assert!(
            body.abs() < 1e-6,
            "body untouched under the threshold: {body}"
        );
    }

    #[test]
    fn past_the_threshold_the_body_is_dragged_along() {
        // Idle, head 80° off: the body is pulled so the head-body offset is
        // exactly the 45° limit.
        let head = 80f32.to_radians();
        let body = follow_body_yaw(0.0, head, false, 0.016);
        assert!(
            (wrap_angle(head - body) - HEAD_YAW_LIMIT).abs() < 1e-5,
            "offset clamps to the limit"
        );
        // Same on the other side.
        let body = follow_body_yaw(0.0, -head, false, 0.016);
        assert!((wrap_angle(-head - body) + HEAD_YAW_LIMIT).abs() < 1e-5);
    }

    #[test]
    fn walking_realigns_the_body_to_the_look() {
        // While moving the body converges to the head across frames, even when
        // the offset never crosses the drag threshold.
        let head = 30f32.to_radians();
        let mut body = 0.0;
        for _ in 0..120 {
            body = follow_body_yaw(body, head, true, 1.0 / 60.0);
        }
        assert!(
            wrap_angle(head - body).abs() < 0.01,
            "body aligned while walking: {body}"
        );
    }

    #[test]
    fn follow_handles_the_yaw_wrap_seam() {
        // Head just past +π, body just under -π: the true offset is tiny, so the
        // body must not spin the long way round.
        let head = std::f32::consts::PI - 0.05;
        let body0 = -std::f32::consts::PI + 0.05;
        let body = follow_body_yaw(body0, head, false, 0.016);
        assert!(
            wrap_angle(head - body).abs() <= HEAD_YAW_LIMIT + 1e-5,
            "wrap seam does not over-rotate"
        );
    }

    #[test]
    fn pose_walk_weight_eases_in_and_back_to_rest() {
        let mut pose = BodyPose::default();
        pose.advance(1.0 / 60.0, 3.0, 0.0, true, false);
        assert!(pose.moving);
        assert!(
            pose.walk_weight > 0.0 && pose.walk_weight < 1.0,
            "the blend eases rather than snapping: {}",
            pose.walk_weight
        );
        let mid_phase = pose.anim_time;
        assert!(mid_phase > 0.0, "the phase advances while moving");
        // Stop: the weight decays smoothly and eventually clamps to rest.
        for _ in 0..120 {
            pose.advance(1.0 / 60.0, 0.0, 0.0, true, false);
        }
        assert_eq!(pose.walk_weight, 0.0, "stopping settles back to rest");
    }

    #[test]
    fn lerp_angle_crosses_the_wrap_seam_the_short_way() {
        use std::f32::consts::PI;
        let mid = lerp_angle(PI - 0.1, -PI + 0.1, 0.5);
        assert!(
            wrap_angle(mid - PI).abs() < 1e-5,
            "midpoint sits on the seam, not the long way round: {mid}"
        );
    }
}
