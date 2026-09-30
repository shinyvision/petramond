use crate::world::ServerWorld;
use petramond_math::math::{IVec3, Vec3};

use super::instance::{hurt_flash01, Instance};
use super::model_meta::Skeleton;
use super::ragdoll::Ragdoll;
use super::{EntityRef, MobDamageFeedback, MobDamageFeedbackComponent, MobDef};

mod pose;

const KNOCKBACK_SPEED: f32 = 6.5;
const KNOCKBACK_UP: f32 = 4.2;

pub(super) enum DeathState {
    Alive,
    NoPresentation,
    Ragdoll(Ragdoll),
}

impl DeathState {
    #[inline]
    pub(super) fn is_dead(&self) -> bool {
        !matches!(self, Self::Alive)
    }

    #[inline]
    pub(super) fn is_despawned(&self) -> bool {
        match self {
            Self::Alive => false,
            Self::NoPresentation => true,
            Self::Ragdoll(ragdoll) => ragdoll.is_done(),
        }
    }
}

impl Instance {
    pub fn damage(
        &mut self,
        amount: f32,
        origin: Option<petramond_math::world_pos::WorldPos>,
        attack: bool,
        attacker: Option<EntityRef>,
        feedback: &MobDamageFeedback,
    ) -> bool {
        if self.combat.death.is_dead()
            || (self.combat.damage_immunity.is_active() && feedback.has_immunity())
        {
            return false;
        }
        let decreases_health = feedback
            .components
            .iter()
            .any(|c| matches!(c, MobDamageFeedbackComponent::DecreaseHealth));
        let lethal = if decreases_health && amount > 0.0 {
            let health = self.health() - amount;
            self.set_health(health);
            for component in &feedback.components {
                if let MobDamageFeedbackComponent::Immunity { ticks } = component {
                    self.combat.damage_immunity.grant_for(*ticks);
                }
            }
            health <= 0.0
        } else {
            false
        };
        if decreases_health && amount > 0.0 {
            if let Some(who) = attacker {
                self.combat.attacker = Some(who);
                self.combat.attacker_ticks = 0;
            }
        }
        if lethal {
            self.set_health(0.0);
        }

        let mut death_pose = (lethal
            && feedback
                .components
                .iter()
                .any(|c| matches!(c, MobDamageFeedbackComponent::Ragdoll { .. })))
        .then(|| self.death_pose());

        for component in &feedback.components {
            match *component {
                MobDamageFeedbackComponent::Immunity { .. } => {}
                MobDamageFeedbackComponent::DecreaseHealth => {}
                MobDamageFeedbackComponent::Flash { duration } => {
                    self.combat.hurt_timer = self.combat.hurt_timer.max(duration.max(0.0));
                }
                MobDamageFeedbackComponent::Knockback { scale, duration } => {
                    if !lethal && attack && scale > 0.0 {
                        if let Some(from) = origin {
                            let mut away = self.pos - from;
                            away.y = 0.0;
                            self.motion.knockback =
                                away.normalize_or_zero() * KNOCKBACK_SPEED * scale;
                            self.motion.vel.y = KNOCKBACK_UP * scale;
                            self.combat.stagger_timer =
                                self.combat.stagger_timer.max(duration.max(0.0));
                            self.motion.on_ground = false;
                        }
                    }
                }
                MobDamageFeedbackComponent::Sound { .. } => {}
                MobDamageFeedbackComponent::Ragdoll {
                    joints,
                    impulse_scale,
                } => {
                    if lethal && matches!(self.combat.death, DeathState::Alive) {
                        let mut away = origin
                            .filter(|_| attack)
                            .map_or(Vec3::ZERO, |from| self.pos - from);
                        away.y = 0.0;
                        let launch = away.normalize_or_zero();
                        self.combat.death = DeathState::Ragdoll(Ragdoll::pending(
                            self.rng.next_u64(),
                            launch,
                            joints,
                            impulse_scale,
                            death_pose.take().unwrap_or_default(),
                        ));
                    }
                }
            }
        }

        if lethal {
            if matches!(self.combat.death, DeathState::Alive) {
                self.combat.death = DeathState::NoPresentation;
            }
            self.motion.knockback = Vec3::ZERO;
            self.combat.stagger_timer = 0.0;
            self.clear_drive();
            self.moving = false;
            self.idle_anim = None;
            return true;
        }
        false
    }

    #[inline]
    pub fn is_dead(&self) -> bool {
        self.combat.death.is_dead()
    }

    #[inline]
    pub fn staggered(&self) -> bool {
        self.combat.stagger_timer > 0.0
    }

    #[inline]
    pub fn health(&self) -> f32 {
        self.tags()
            .get(super::tags::HEALTH)
            .and_then(super::MobTagValue::as_float)
            .map_or_else(|| super::def(self.kind).spawn_health(), |h| h as f32)
    }

    #[inline]
    pub(super) fn set_health(&mut self, health: f32) {
        self.tags_mut().insert(
            super::tags::HEALTH.to_owned(),
            super::MobTagValue::Float(health.max(0.0) as f64),
        );
    }

    #[inline]
    pub fn is_damage_immune(&self) -> bool {
        self.combat.damage_immunity.is_active()
    }

    #[inline]
    pub(super) fn tick_damage_immunity(&mut self) {
        self.combat.damage_immunity.tick();
    }

    #[inline]
    pub fn is_despawned(&self) -> bool {
        self.combat.death.is_despawned()
    }

    #[inline]
    pub fn is_distance_despawned(&self) -> bool {
        self.distance_despawned
    }

    pub fn hurt_flash(&self, alpha: f32) -> f32 {
        hurt_flash01(self.interp.hurt, self.combat.hurt_timer, alpha)
    }

    #[inline]
    pub fn hurt_timer(&self) -> f32 {
        self.combat.hurt_timer
    }

    pub fn ragdoll_pose(&self, alpha: f32) -> Option<Vec<(Vec3, glam::Quat)>> {
        let DeathState::Ragdoll(rag) = &self.combat.death else {
            return None;
        };
        if !rag.is_initialized() {
            return Some(rag.pending_pose(super::model(self.kind)));
        }
        Some(rag.pose(alpha))
    }

    pub(super) fn tick_ragdoll(
        &mut self,
        dt: f32,
        world: &ServerWorld,
        d: &MobDef,
        skeleton: &Skeleton,
    ) {
        let vel = self.motion.vel;
        let yaw = self.yaw;
        let pos = self.pos;
        let DeathState::Ragdoll(rag) = &mut self.combat.death else {
            return;
        };
        if rag.is_initialized() {
            let anchor = pos.block();
            let solid = |c: IVec3| {
                let w = c + anchor;
                world.data().blocks_movement_at(w.x, w.y, w.z)
            };
            rag.step(dt, d.scale, pos.relative_to(anchor), yaw, &solid);
        } else {
            rag.init(skeleton, d.scale, vel, yaw);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mob::{def, Mob};
    use petramond_math::world_pos::WorldPos;

    fn floor_at_zero(p: IVec3) -> bool {
        p.y < 0
    }

    fn owl_def() -> &'static MobDef {
        def(Mob::Owl)
    }

    fn default_feedback() -> MobDamageFeedback {
        MobDamageFeedback::default()
    }

    #[test]
    fn lethal_damage_discards_a_pending_drive_intent() {
        let mut owl = Instance::new(Mob::Owl, WorldPos::new(0.5, 0.0, 0.5), 0.0, 1);
        assert!(owl.set_drive(crate::mob::kinematics::DriveIntent {
            horizontal: Some([2.0, 0.0]),
            vertical: None,
            yaw: Some(1.0),
            while_walking: false,
            gait: false,
        }));
        assert!(owl.drive_pending());
        assert!(owl.damage(
            100.0,
            Some(WorldPos::new(2.0, 0.0, 0.5)),
            true,
            None,
            &default_feedback()
        ));
        assert!(!owl.drive_pending());
    }

    #[test]
    fn damage_reduces_health_and_dies_at_zero() {
        let mut owl = Instance::new(Mob::Owl, WorldPos::new(0.5, 0.0, 0.5), 0.0, 1);
        let from = WorldPos::new(5.0, 0.0, 0.5);
        for _ in 0..3 {
            assert!(!owl.damage(1.0, Some(from), true, None, &default_feedback()));
            for _ in 0..petramond_world::damage::MOB_DAMAGE_IFRAME_TICKS {
                owl.tick_damage_immunity();
            }
        }
        assert!(!owl.is_dead(), "still alive at 1 health");
        assert!(
            owl.damage(1.0, Some(from), true, None, &default_feedback()),
            "the lethal hit reports true"
        );
        assert!(owl.is_dead(), "dead at 0 health");
    }

    #[test]
    fn empty_damage_feedback_does_nothing() {
        let mut owl = Instance::new(Mob::Owl, WorldPos::new(0.5, 0.0, 0.5), 0.0, 1);
        owl.integrate(0.05, owl_def(), Vec3::ZERO, false, &floor_at_zero);
        let health = owl.health();
        let x0 = owl.pos.x;

        assert!(!owl.damage(
            100.0,
            Some(WorldPos::new(5.0, 0.0, 0.5)),
            true,
            None,
            &MobDamageFeedback::none()
        ));
        assert_eq!(owl.health(), health);
        assert!(!owl.is_dead());
        assert_eq!(owl.hurt_flash(1.0), 0.0);

        owl.integrate(0.05, owl_def(), Vec3::ZERO, false, &floor_at_zero);
        assert!(
            (owl.pos.x - x0).abs() < 1e-4,
            "empty feedback should not apply knockback: {x0} -> {}",
            owl.pos.x
        );
    }

    #[test]
    fn ragdoll_feedback_is_death_gated() {
        let mut ragdoll_only = Instance::new(Mob::Owl, WorldPos::new(0.5, 0.0, 0.5), 0.0, 1);
        let ragdoll = MobDamageFeedback {
            components: vec![MobDamageFeedbackComponent::Ragdoll {
                joints: crate::mob::RagdollJoints::Connected,
                impulse_scale: 1.0,
            }],
        };
        assert!(!ragdoll_only.damage(
            100.0,
            Some(WorldPos::new(5.0, 0.0, 0.5)),
            true,
            None,
            &ragdoll
        ));
        assert!(
            !ragdoll_only.is_dead(),
            "ragdoll alone cannot kill without health feedback"
        );

        let mut dead_with_ragdoll = Instance::new(Mob::Owl, WorldPos::new(0.5, 0.0, 0.5), 0.0, 1);
        let health_and_ragdoll = MobDamageFeedback {
            components: vec![
                MobDamageFeedbackComponent::DecreaseHealth,
                MobDamageFeedbackComponent::Ragdoll {
                    joints: crate::mob::RagdollJoints::Connected,
                    impulse_scale: 1.0,
                },
            ],
        };
        assert!(dead_with_ragdoll.damage(
            100.0,
            Some(WorldPos::new(5.0, 0.0, 0.5)),
            true,
            None,
            &health_and_ragdoll
        ));
        assert!(dead_with_ragdoll.is_dead());
        assert!(
            !dead_with_ragdoll.is_despawned(),
            "ragdoll presentation keeps the corpse until the ragdoll finishes"
        );

        let mut dead_without_ragdoll =
            Instance::new(Mob::Owl, WorldPos::new(0.5, 0.0, 0.5), 0.0, 1);
        let health_only = MobDamageFeedback {
            components: vec![MobDamageFeedbackComponent::DecreaseHealth],
        };
        assert!(dead_without_ragdoll.damage(
            100.0,
            Some(WorldPos::new(5.0, 0.0, 0.5)),
            true,
            None,
            &health_only
        ));
        assert!(dead_without_ragdoll.is_dead());
        assert!(
            dead_without_ragdoll.is_despawned(),
            "without a death presentation component, the dead mob has no corpse to keep"
        );
    }

    #[test]
    fn a_dead_mob_ignores_further_damage() {
        let mut owl = Instance::new(Mob::Owl, WorldPos::new(0.5, 0.0, 0.5), 0.0, 1);
        assert!(
            owl.damage(
                100.0,
                Some(WorldPos::new(5.0, 0.0, 0.5)),
                true,
                None,
                &default_feedback()
            ),
            "one big hit kills"
        );
        assert!(!owl.damage(
            100.0,
            Some(WorldPos::new(5.0, 0.0, 0.5)),
            true,
            None,
            &default_feedback()
        ));
        assert!(owl.is_dead());
    }

    #[test]
    fn non_attack_damage_does_not_apply_default_knockback() {
        let mut owl = Instance::new(Mob::Owl, WorldPos::new(0.5, 0.0, 0.5), 0.0, 1);
        owl.integrate(0.05, owl_def(), Vec3::ZERO, false, &floor_at_zero);
        let x0 = owl.pos.x;
        assert!(!owl.damage(
            1.0,
            Some(WorldPos::new(5.0, 0.0, 0.5)),
            false,
            None,
            &default_feedback()
        ));
        owl.integrate(0.05, owl_def(), Vec3::ZERO, false, &floor_at_zero);
        assert!(
            (owl.pos.x - x0).abs() < 1e-4,
            "non-attack damage should not shove the mob: {x0} -> {}",
            owl.pos.x
        );
    }

    #[test]
    fn every_hit_flashes_red_including_the_kill() {
        let mut owl = Instance::new(Mob::Owl, WorldPos::new(0.5, 0.0, 0.5), 0.0, 1);
        owl.damage(
            1.0,
            Some(WorldPos::new(5.0, 0.0, 0.5)),
            true,
            None,
            &default_feedback(),
        );
        assert!(owl.hurt_flash(1.0) > 0.0, "a non-lethal hit flashes red");

        let mut dead = Instance::new(Mob::Owl, WorldPos::new(0.5, 0.0, 0.5), 0.0, 1);
        assert!(dead.damage(
            100.0,
            Some(WorldPos::new(5.0, 0.0, 0.5)),
            true,
            None,
            &default_feedback()
        ));
        assert!(
            dead.hurt_flash(1.0) > 0.0,
            "the kill flashes red like a normal hit"
        );
        assert!(
            dead.ragdoll_pose(0.5).is_some(),
            "the pose at death is available before physics starts"
        );
    }
}
