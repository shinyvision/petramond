use petramond_math::math::Vec3;

use super::super::brain::{AiBehavior, AiCtx, BehaviorOutput, HeadLook};

const LOOK_RADIUS: f32 = 2.0;
const LOOK_AT_PLAYER_CHANCE: f32 = 0.85;
const MAX_HEAD_YAW: f32 = std::f32::consts::FRAC_PI_2;
const MAX_HEAD_PITCH: f32 = 0.9;
const GLANCE_YAW: f32 = 1.1;
const GLANCE_PITCH: f32 = 0.25;
const REPICK_MIN_TICKS: u32 = 20;
const REPICK_SPAN_TICKS: u32 = 40;

enum LookMode {
    AtPlayer,
    Glance(HeadLook),
}

pub struct HeadLookAi {
    mode: LookMode,
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
        if !ctx.nav_idle {
            self.timer = 0;
            return BehaviorOutput::default();
        }
        if self.timer == 0 {
            self.mode = pick_mode(ctx);
            self.timer = REPICK_MIN_TICKS + (ctx.rng.next_f32() * REPICK_SPAN_TICKS as f32) as u32;
        } else {
            self.timer -= 1;
        }
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

fn look_at_player(ctx: &AiCtx) -> Option<HeadLook> {
    head_look_toward(ctx.player_pos - head_pos(ctx), ctx.yaw)
}

fn head_look_toward(to: Vec3, body_yaw: f32) -> Option<HeadLook> {
    let horiz = (to.x * to.x + to.z * to.z).sqrt();
    let world_yaw = (-to.x).atan2(-to.z);
    let yaw = wrap_angle(world_yaw - body_yaw);
    if yaw.abs() > MAX_HEAD_YAW {
        return None;
    }
    let pitch =
        to.y.atan2(horiz.max(1e-3))
            .clamp(-MAX_HEAD_PITCH, MAX_HEAD_PITCH);
    Some(HeadLook { yaw, pitch })
}

fn head_pos(ctx: &AiCtx) -> petramond_math::world_pos::WorldPos {
    ctx.pos + Vec3::new(0.0, ctx.head_height, 0.0)
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
        let target = Vec3::new(0.0, 0.0, -2.0);
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
        let h = head_look_toward(Vec3::new(0.0, 5.0, -0.1), 0.0).expect("in front is lookable");
        assert!(h.pitch > 0.0, "looks up at a higher target: {}", h.pitch);
        assert!(
            h.pitch <= MAX_HEAD_PITCH + 1e-6,
            "pitch is capped: {}",
            h.pitch
        );
    }
}
