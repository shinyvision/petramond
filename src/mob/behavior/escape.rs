//! Short, changing escape legs shared by `panic` and defensive `retaliate`.
//!
//! An escaping mob does not run in a straight line: it picks a reachable
//! foothold a few blocks away, off the dead-away bearing, alternating sides,
//! holds that leg for under a second, then picks again. That reads as an
//! animal bolting rather than a body sliding to the horizon, and it keeps the
//! mob out of the corner a straight retreat would pin it in.

use petramond_math::math::IVec3;

use super::super::brain::{AiCtx, ChannelClaims, DecisionChannel};
use super::super::nav;
use super::super::path::is_navigation_foothold_with;

/// Default escape reach (blocks): enough to leave melee range in one leg
/// without pathing across a whole clearing.
const DEFAULT_RADIUS: u32 = 7;
/// Accepted `radius` values: below 3 every leg collides with the minimum leg
/// length; above 16 a leg leaves the cells the navigator can plan across.
const RADIUS_BOUNDS: std::ops::RangeInclusive<u32> = 3..=16;
/// A leg is never shorter than this (blocks) — anything less stops and
/// restarts inside one gait cycle.
const LEG_MIN_DISTANCE: f32 = 2.0;
/// A leg bears at least this far (radians) off the dead-away direction, and
/// at most `LEG_ANGLE_MIN + LEG_ANGLE_SPREAD`: never straight away (predictable,
/// and corners the mob) and never back toward the threat.
const LEG_ANGLE_MIN: f32 = 0.35;
const LEG_ANGLE_SPREAD: f32 = 1.15;
/// Candidate legs rolled before giving up for this attempt.
const LEG_ATTEMPTS: u32 = 12;
/// Vertical offsets tried for a foothold at the rolled column, nearest first.
const FOOTHOLD_DY: [i32; 5] = [0, 1, -1, 2, -2];
/// A leg must END at least this much farther (blocks) from the threat than
/// the mob stands now, or it is a sidestep, not an escape.
const AWAY_MARGIN: f32 = 0.5;
/// How long (ticks) a chosen leg is held before re-planning: 0.7–1.4 s at
/// 20 TPS, jittered so a herd does not turn in lockstep.
const HOLD_TICKS_MIN: u32 = 14;
const HOLD_TICKS_JITTER: u32 = 14;
/// Ticks before trying again after no leg was found (or the reach budget
/// refused) — long enough not to burn the shared probe budget every tick.
const RETRY_TICKS: u32 = 8;
/// Unreachable candidates tolerated per attempt before deferring; each one
/// spent a reachability probe from the tick-wide budget.
const MAX_UNREACHABLE_PROBES: u32 = 3;
/// Below this horizontal offset (blocks) the mob stands ON the threat and
/// "away" has no direction; a random bearing is used instead.
const MIN_THREAT_OFFSET: f32 = 0.01;

/// The `radius` a `panic` or `retaliate` brain row may set (blocks): both
/// nodes accept this one key with one default and one range, and hand it
/// here. (Each row struct carries the key itself — serde cannot flatten into
/// a `deny_unknown_fields` struct — so this is the single validation point.)
#[derive(Copy, Clone, Default)]
pub(super) struct EscapeParams {
    radius: Option<u32>,
}

impl EscapeParams {
    /// `None` = the default radius.
    pub fn with_radius(radius: Option<u32>) -> Self {
        EscapeParams { radius }
    }
}

/// One escape planner: the current leg, how long it is held, and which side
/// the next leg bears to.
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
    /// Channels an escaping node holds shut while it runs: no destination
    /// from a lower node (a boxed-in mob STANDS rather than wandering off),
    /// no lock, no strike — a fleeing body neither hunts nor bites.
    pub const HOLDS: ChannelClaims = ChannelClaims::of(&[
        DecisionChannel::Goal,
        DecisionChannel::Target,
        DecisionChannel::Attack,
    ]);

    /// Build from a brain row's escape params, validating the radius.
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

    /// Forget the current leg (the threat changed or ended).
    pub fn reset(&mut self) {
        *self = Self::with_radius(self.radius);
    }

    /// The foothold to head for this tick while escaping `threat`, or `None`
    /// when no reachable leg exists right now (stand; try again shortly).
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
                    // Budget exhausted: defer, never treat the refusal as a verdict.
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
