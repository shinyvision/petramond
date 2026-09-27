use mod_api::MAX_MOB_SPEED_SCALE;
use serde::Deserialize;

use super::super::brain::{AiBehavior, AiCtx, BehaviorOutput, ChannelClaims, DecisionChannel};
use super::super::EntityRef;
use super::escape::{EscapeParams, EscapeRoute};

const DEFAULT_MEMORY_TICKS: u32 = 120;
const DEFAULT_SPEED_SCALE: f32 = 2.0;

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct PanicParams {
    memory_ticks: u32,
    radius: Option<u32>,
    speed_scale: f32,
}

impl Default for PanicParams {
    fn default() -> Self {
        Self {
            memory_ticks: DEFAULT_MEMORY_TICKS,
            radius: None,
            speed_scale: DEFAULT_SPEED_SCALE,
        }
    }
}

pub struct PanicAi {
    memory_ticks: u32,
    speed_scale: f32,
    attacker: Option<EntityRef>,
    route: EscapeRoute,
}

impl PanicAi {
    const HOLDS: ChannelClaims = EscapeRoute::HOLDS.with(DecisionChannel::Facing);

    pub(super) fn from_params(params: &serde_json::Value) -> Result<Self, String> {
        let p: PanicParams = serde_json::from_value(params.clone()).map_err(|e| e.to_string())?;
        if p.memory_ticks == 0 {
            return Err("panic memory_ticks must be >= 1".into());
        }
        if !p.speed_scale.is_finite() || !(0.0..=MAX_MOB_SPEED_SCALE).contains(&p.speed_scale) {
            return Err(format!(
                "panic speed_scale must be finite and in 0..={MAX_MOB_SPEED_SCALE}"
            ));
        }
        let route = EscapeRoute::from_params(EscapeParams::with_radius(p.radius))?;
        Ok(Self {
            memory_ticks: p.memory_ticks,
            speed_scale: p.speed_scale,
            attacker: None,
            route,
        })
    }
}

impl AiBehavior for PanicAi {
    fn tick(&mut self, ctx: &mut AiCtx) -> BehaviorOutput {
        let threat = ctx
            .attacker
            .filter(|(_, age)| *age <= self.memory_ticks)
            .and_then(|(who, _)| ctx.entity_pos(who).map(|pos| (who, pos)));
        let Some((who, pos)) = threat else {
            self.attacker = None;
            self.route.reset();
            return BehaviorOutput::default();
        };
        if self.attacker != Some(who) {
            self.attacker = Some(who);
            self.route.reset();
        }
        BehaviorOutput {
            goal: self.route.goal(ctx, pos),
            speed_scale: Some(self.speed_scale),
            claims: Self::HOLDS,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests;
