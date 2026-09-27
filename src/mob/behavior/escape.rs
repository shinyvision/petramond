use petramond_math::math::IVec3;

use super::super::brain::{AiCtx, ChannelClaims, DecisionChannel};
use super::super::nav;
use super::super::path::is_navigation_foothold_with;

const DEFAULT_RADIUS: u32 = 7;
const RADIUS_BOUNDS: std::ops::RangeInclusive<u32> = 3..=16;
const LEG_MIN_DISTANCE: f32 = 2.0;
const LEG_ANGLE_MIN: f32 = 0.35;
const LEG_ANGLE_SPREAD: f32 = 1.15;
const LEG_ATTEMPTS: u32 = 12;
const FOOTHOLD_DY: [i32; 5] = [0, 1, -1, 2, -2];
const AWAY_MARGIN: f32 = 0.5;
const HOLD_TICKS_MIN: u32 = 14;
const HOLD_TICKS_JITTER: u32 = 14;
const RETRY_TICKS: u32 = 8;
const MAX_UNREACHABLE_PROBES: u32 = 3;
const MIN_THREAT_OFFSET: f32 = 0.01;

#[derive(Copy, Clone, Default)]
pub(super) struct EscapeParams {
    radius: Option<u32>,
}

impl EscapeParams {
    pub fn with_radius(radius: Option<u32>) -> Self {
        EscapeParams { radius }
    }
}

pub(super) struct EscapeRoute {
    radius: u32,
    goal: Option<IVec3>,
    ticks: u32,
    side: bool,
}

impl Default for EscapeRoute {
    fn default() -> Self {
        Self::with_radius(DEFAULT_RADIUS)
    }
}

impl EscapeRoute {
    pub const HOLDS: ChannelClaims = ChannelClaims::of(&[
        DecisionChannel::Goal,
        DecisionChannel::Target,
        DecisionChannel::Attack,
    ]);

    pub fn from_params(params: EscapeParams) -> Result<Self, String> {
        let radius = params.radius.unwrap_or(DEFAULT_RADIUS);
        if !RADIUS_BOUNDS.contains(&radius) {
            return Err(format!(
                "radius must be in {}..={}",
                RADIUS_BOUNDS.start(),
                RADIUS_BOUNDS.end()
            ));
        }
        Ok(Self::with_radius(radius))
    }

    fn with_radius(radius: u32) -> Self {
        EscapeRoute {
            radius,
            goal: None,
            ticks: 0,
            side: false,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::with_radius(self.radius);
    }

    pub fn goal(
        &mut self,
        ctx: &mut AiCtx,
        threat: petramond_math::world_pos::WorldPos,
    ) -> Option<IVec3> {
        self.ticks = self.ticks.saturating_sub(1);
        let safe_goal = self.goal.is_some_and(|g| {
            let step = petramond_math::world_pos::WorldPos::new(
                f64::from(g.x) + 0.5,
                ctx.pos.y,
                f64::from(g.z) + 0.5,
            );
            horizontal_distance(step, threat) > horizontal_distance(ctx.pos, threat)
        });
        if self.ticks > 0 && safe_goal && !ctx.nav_idle {
            return self.goal;
        }
        if self.ticks > 0 && self.goal.is_none() {
            return None;
        }
        self.goal = None;
        self.ticks = 0;
        self.side = !self.side;
        let delta = ctx.pos - threat;
        let away = if delta.x.hypot(delta.z) > MIN_THREAT_OFFSET {
            delta.x.atan2(delta.z)
        } else {
            ctx.rng.next_f32() * std::f32::consts::TAU
        };
        let cursor = ctx.world.cursor();
        let params = ctx.path_params();
        let solid = nav::nav_solid_fn(&cursor);
        let support = nav::nav_support_fn(&cursor, params.half_width);
        let fluid = nav::nav_fluid_fn(&cursor);
        let mut probes = 0;
        for attempt in 0..LEG_ATTEMPTS {
            let sign = if self.side ^ (attempt % 2 == 1) {
                1.0
            } else {
                -1.0
            };
            let angle = away + sign * (LEG_ANGLE_MIN + ctx.rng.next_f32() * LEG_ANGLE_SPREAD);
            let distance =
                LEG_MIN_DISTANCE + ctx.rng.next_f32() * (self.radius as f32 - LEG_MIN_DISTANCE);
            let x = (ctx.pos.x + f64::from(angle.sin() * distance)).floor() as i32;
            let z = (ctx.pos.z + f64::from(angle.cos() * distance)).floor() as i32;
            for dy in FOOTHOLD_DY {
                let goal = IVec3::new(x, ctx.cell.y + dy, z);
                if !is_navigation_foothold_with(goal, params, &solid, &support, &fluid) {
                    continue;
                }
                let end = petramond_math::world_pos::WorldPos::new(
                    f64::from(x) + 0.5,
                    ctx.pos.y,
                    f64::from(z) + 0.5,
                );
                if horizontal_distance(end, threat)
                    <= horizontal_distance(ctx.pos, threat) + AWAY_MARGIN
                {
                    continue;
                }
                match nav::destination_reachable(
                    ctx.world,
                    ctx.cell,
                    goal,
                    params,
                    ctx.head_height,
                    ctx.reach,
                ) {
                    None => return None,
                    Some(true) => {
                        self.goal = Some(goal);
                        self.ticks =
                            HOLD_TICKS_MIN + (ctx.rng.next_f32() * HOLD_TICKS_JITTER as f32) as u32;
                        return self.goal;
                    }
                    Some(false) => probes += 1,
                }
                if probes >= MAX_UNREACHABLE_PROBES {
                    self.ticks = RETRY_TICKS;
                    return None;
                }
                break;
            }
        }
        self.ticks = RETRY_TICKS;
        None
    }
}

fn horizontal_distance(
    a: petramond_math::world_pos::WorldPos,
    b: petramond_math::world_pos::WorldPos,
) -> f32 {
    let d = a - b;
    d.x.hypot(d.z)
}
