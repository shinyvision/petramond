use super::game::ServerGame;
use crate::events::{tick::TickEvents, DamageSource};
use crate::mob::{MobDamageFeedback, MobDamageFeedbackComponent, MobExposureDamage};
use petramond_world::damage::Immunity;
use petramond_world::exposure::{ExposureDamage, ExposureSource};

impl ServerGame {
    pub(crate) fn tick_player_exposure(&mut self, s: usize, events: &mut TickEvents) {
        let player = &mut self.sessions[s].player;
        if player.is_invulnerable() || player.health() == 0 {
            player.clear_exposure();
            return;
        }
        let mut scratch = std::mem::take(&mut self.exposure_scratch);
        scratch.tick(&self.world, [player.body().aabb()], player.exposure_mut());
        for hit in scratch.damage.drain(..) {
            self.damage_player_through_funnel(
                s,
                hit.amount,
                source(hit),
                None,
                Immunity::Exempt,
                events,
            );
        }
        self.exposure_scratch = scratch;
    }

    pub(crate) fn apply_mob_exposure_damage(
        &mut self,
        hits: Vec<MobExposureDamage>,
        events: &mut TickEvents,
    ) {
        for hit in hits {
            let Some(mob) = self.world.mobs().get(hit.mob_id) else {
                continue;
            };
            let species = &crate::mob::def(mob.kind).damage_feedback;
            self.damage_mob_through_pipeline(
                hit.mob_id,
                hit.damage.amount as f32,
                source(hit.damage),
                None,
                Some(clocked_feedback(species)),
                events,
            );
        }
    }
}

fn source(hit: ExposureDamage) -> DamageSource {
    match hit.source {
        ExposureSource::Fluid(block) => DamageSource::Fluid(block),
        ExposureSource::Condition(condition) => DamageSource::Condition(condition),
    }
}

fn clocked_feedback(species: &MobDamageFeedback) -> MobDamageFeedback {
    let mut feedback = species.clone();
    feedback.components.retain(|c| match c {
        MobDamageFeedbackComponent::Immunity { .. }
        | MobDamageFeedbackComponent::Knockback { .. } => false,
        MobDamageFeedbackComponent::DecreaseHealth
        | MobDamageFeedbackComponent::Flash { .. }
        | MobDamageFeedbackComponent::Sound { .. }
        | MobDamageFeedbackComponent::Ragdoll { .. } => true,
    });
    feedback
}

#[cfg(test)]
mod tests;
