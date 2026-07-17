//! Instance damage intake and the death lifecycle: row/hook-composed damage
//! feedback, hurt flash, damage immunity, retaliation memory recording, the
//! death state (ragdoll or bare), and the despawn queries the manager culls by.

use crate::mathh::{IVec3, Vec3};
use crate::world::World;

use super::instance::{hurt_flash01, Instance};
use super::model_meta::Skeleton;
use super::ragdoll::Ragdoll;
use super::{EntityRef, MobDamageFeedback, MobDamageFeedbackComponent, MobDef};

/// Horizontal speed (m/s) imparted away from the attacker on a non-lethal hit.
const KNOCKBACK_SPEED: f32 = 6.5;
/// One-shot upward pop (m/s) on a non-lethal hit — a small hop, like a soft jump.
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
    /// Apply a damage request with row/hook-composed feedback. Returns `true` if this
    /// hit was lethal. A dead mob ignores damage (no double-kill, no knockback on a
    /// corpse). `petramond:ragdoll` is death-gated: it only starts the ragdoll if a
    /// `petramond:decrease_health` component made this hit cross to zero.
    ///
    /// `attacker` is the entity that caused the hit, when the source names one —
    /// recorded as this mob's retaliation memory (see the `retaliate` brain node).
    /// Whether the species reacts is brain data; the record itself is generic.
    ///
    /// The i-frame window is a pipeline component (`petramond:immunity`): a
    /// pipeline carrying it is blocked while a window is active and grants
    /// its `ticks` on a real health decrease; a pipeline without it (DoT —
    /// burn ticks) neither blocks nor grants.
    pub fn damage(
        &mut self,
        amount: f32,
        origin: Option<Vec3>,
        attack: bool,
        attacker: Option<EntityRef>,
        feedback: &MobDamageFeedback,
    ) -> bool {
        if self.death.is_dead() || (self.damage_immunity.is_active() && feedback.has_immunity()) {
            return false;
        }
        let decreases_health = feedback
            .components
            .iter()
            .any(|c| matches!(c, MobDamageFeedbackComponent::DecreaseHealth));
        let lethal = if decreases_health && amount > 0.0 {
            self.health -= amount;
            for component in &feedback.components {
                if let MobDamageFeedbackComponent::Immunity { ticks } = component {
                    self.damage_immunity.grant_for(*ticks);
                }
            }
            self.health <= 0.0
        } else {
            false
        };
        if decreases_health && amount > 0.0 {
            if let Some(who) = attacker {
                self.attacker = Some(who);
                self.attacker_ticks = 0;
            }
        }
        if lethal {
            self.health = 0.0;
        }

        for component in &feedback.components {
            match *component {
                // Applied above with the health decrease (grant-on-hit).
                MobDamageFeedbackComponent::Immunity { .. } => {}
                MobDamageFeedbackComponent::DecreaseHealth => {}
                MobDamageFeedbackComponent::Flash { duration } => {
                    self.hurt_timer = self.hurt_timer.max(duration.max(0.0));
                }
                MobDamageFeedbackComponent::Knockback { scale, duration } => {
                    if !lethal && attack && scale > 0.0 {
                        if let Some(from) = origin {
                            let mut away = self.pos - from;
                            away.y = 0.0;
                            self.knockback = away.normalize_or_zero() * KNOCKBACK_SPEED * scale;
                            self.vel.y = KNOCKBACK_UP * scale;
                            self.stagger_timer = self.stagger_timer.max(duration.max(0.0));
                            self.on_ground = false;
                        }
                    }
                }
                MobDamageFeedbackComponent::Sound { .. } => {}
                MobDamageFeedbackComponent::Ragdoll => {
                    if lethal && matches!(self.death, DeathState::Alive) {
                        // The killing blow flings the corpse in the punched direction
                        // (away from the attacker, horizontally); the ragdoll launches
                        // + somersaults along it.
                        let mut away = origin
                            .filter(|_| attack)
                            .map_or(Vec3::ZERO, |from| self.pos - from);
                        away.y = 0.0;
                        let launch = away.normalize_or_zero();
                        // Ragdoll is initialised on the next tick (which has world
                        // access to find the floor). Seed it from this mob's RNG
                        // stream for a distinct fling.
                        self.death =
                            DeathState::Ragdoll(Ragdoll::pending(self.rng.next_u64(), launch));
                    }
                }
            }
        }

        if lethal {
            if matches!(self.death, DeathState::Alive) {
                self.death = DeathState::NoPresentation;
            }
            self.knockback = Vec3::ZERO;
            self.stagger_timer = 0.0;
            self.drive = None;
            self.moving = false;
            self.idle_anim = None;
            return true;
        }
        false
    }

    /// Is the mob dead (ragdolling or done)? A dead mob can't be targeted or hurt.
    #[inline]
    pub fn is_dead(&self) -> bool {
        self.death.is_dead()
    }

    /// Current health (`0` = dead), for mod `MobsInRadius` snapshots.
    #[inline]
    pub fn health(&self) -> f32 {
        self.health
    }

    #[inline]
    pub(crate) fn is_damage_immune(&self) -> bool {
        self.damage_immunity.is_active()
    }

    #[inline]
    pub(super) fn tick_damage_immunity(&mut self) {
        self.damage_immunity.tick();
    }

    /// Has the death ragdoll finished, so the corpse should be removed from the world?
    #[inline]
    pub fn is_despawned(&self) -> bool {
        self.death.is_despawned()
    }

    /// Has this mob moved beyond its row-level despawn radius and should be culled at
    /// the end of this tick? Always false for species that never distance-despawn. The
    /// manager drops such a mob from the live set without saving it — distinct from
    /// [`is_despawned`](Self::is_despawned), which is a finished death corpse.
    #[inline]
    pub fn is_distance_despawned(&self) -> bool {
        self.distance_despawned
    }

    /// Hurt-flash intensity in `[0, 1]` at `alpha` into the tick. The renderer fades the
    /// red tint by this. Applies while dying too (the flash from the killing blow), so a
    /// kill reads like any other hit; it decays to 0 over the start of the ragdoll.
    pub fn hurt_flash(&self, alpha: f32) -> f32 {
        hurt_flash01(self.prev_hurt, self.hurt_timer, alpha)
    }

    /// The remaining hurt stagger/flash timer (seconds) — the SOURCE state the
    /// flash derives from. Replicated per tick; the client derives the flash
    /// from consecutive values via [`hurt_flash01`].
    #[inline]
    pub fn hurt_timer(&self) -> f32 {
        self.hurt_timer
    }

    /// The interpolated per-bone ragdoll pose (pivot position + orientation) at `alpha`,
    /// or `None` if the mob isn't ragdolling yet. The renderer builds each bone's pose
    /// as `T(pos)·R(rot)·T(-pivot)`.
    pub fn ragdoll_pose(&self, alpha: f32) -> Option<Vec<(Vec3, glam::Quat)>> {
        let DeathState::Ragdoll(rag) = &self.death else {
            return None;
        };
        if !rag.is_initialized() {
            return None;
        }
        Some(rag.pose(alpha))
    }

    /// Advance the death ragdoll. On its first dead tick the ragdoll is initialised;
    /// thereafter it steps, colliding each bone-corner against the world's blocks (so the
    /// corpse can't pass through terrain and falls off edges). The mob's `pos`/`yaw` stay
    /// frozen — they're the ragdoll's model→world `global` transform.
    pub(super) fn tick_ragdoll(&mut self, dt: f32, world: &World, d: &MobDef, skeleton: &Skeleton) {
        let vel = self.vel;
        let yaw = self.yaw;
        let pos = self.pos;
        let DeathState::Ragdoll(rag) = &mut self.death else {
            return;
        };
        if rag.is_initialized() {
            let solid = |c: IVec3| world.blocks_movement_at(c.x, c.y, c.z);
            rag.step(dt, d.scale, pos, yaw, &solid);
        } else {
            rag.init(skeleton, d.scale, vel, yaw);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mob::{def, Mob};

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
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
        assert!(owl.set_drive(2.0, 0.0, Some(1.0)));
        assert!(owl.drive_pending());
        assert!(owl.damage(100.0,
            Some(Vec3::new(2.0, 0.0, 0.5)),
            true,
            None, &default_feedback()));
        assert!(!owl.drive_pending());
    }

    #[test]
    fn damage_reduces_health_and_dies_at_zero() {
        // A 4-health owl: three 1-damage hits don't kill; the fourth does.
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
        let from = Vec3::new(5.0, 0.0, 0.5);
        for _ in 0..3 {
            assert!(!owl.damage(1.0, Some(from), true, None, &default_feedback()));
            for _ in 0..crate::damage::MOB_DAMAGE_IFRAME_TICKS {
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
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
        owl.integrate(0.05, owl_def(), Vec3::ZERO, false, &floor_at_zero, &|_| {
            false
        });
        let health = owl.health();
        let x0 = owl.pos.x;

        assert!(!owl.damage(
            100.0,
            Some(Vec3::new(5.0, 0.0, 0.5)),
            true,
            None,
            &MobDamageFeedback::none()
        ));
        assert_eq!(owl.health(), health);
        assert!(!owl.is_dead());
        assert_eq!(owl.hurt_flash(1.0), 0.0);

        owl.integrate(0.05, owl_def(), Vec3::ZERO, false, &floor_at_zero, &|_| {
            false
        });
        assert!(
            (owl.pos.x - x0).abs() < 1e-4,
            "empty feedback should not apply knockback: {x0} -> {}",
            owl.pos.x
        );
    }

    #[test]
    fn ragdoll_feedback_is_death_gated() {
        let mut ragdoll_only = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
        let ragdoll = MobDamageFeedback {
            components: vec![MobDamageFeedbackComponent::Ragdoll],
        };
        assert!(!ragdoll_only.damage(100.0, Some(Vec3::new(5.0, 0.0, 0.5)), true, None, &ragdoll));
        assert!(
            !ragdoll_only.is_dead(),
            "ragdoll alone cannot kill without health feedback"
        );

        let mut dead_with_ragdoll = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
        let health_and_ragdoll = MobDamageFeedback {
            components: vec![
                MobDamageFeedbackComponent::DecreaseHealth,
                MobDamageFeedbackComponent::Ragdoll,
            ],
        };
        assert!(dead_with_ragdoll.damage(100.0,
            Some(Vec3::new(5.0, 0.0, 0.5)),
            true,
            None, &health_and_ragdoll));
        assert!(dead_with_ragdoll.is_dead());
        assert!(
            !dead_with_ragdoll.is_despawned(),
            "ragdoll presentation keeps the corpse until the ragdoll finishes"
        );

        let mut dead_without_ragdoll = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
        let health_only = MobDamageFeedback {
            components: vec![MobDamageFeedbackComponent::DecreaseHealth],
        };
        assert!(dead_without_ragdoll.damage(100.0,
            Some(Vec3::new(5.0, 0.0, 0.5)),
            true,
            None, &health_only));
        assert!(dead_without_ragdoll.is_dead());
        assert!(
            dead_without_ragdoll.is_despawned(),
            "without a death presentation component, the dead mob has no corpse to keep"
        );
    }

    #[test]
    fn a_dead_mob_ignores_further_damage() {
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
        assert!(
            owl.damage(100.0,
                Some(Vec3::new(5.0, 0.0, 0.5)),
                true,
                None, &default_feedback()),
            "one big hit kills"
        );
        // A corpse takes no more damage and reports no further lethal hits.
        assert!(!owl.damage(100.0,
            Some(Vec3::new(5.0, 0.0, 0.5)),
            true,
            None, &default_feedback()));
        assert!(owl.is_dead());
    }

    #[test]
    fn non_attack_damage_does_not_apply_default_knockback() {
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
        owl.integrate(0.05, owl_def(), Vec3::ZERO, false, &floor_at_zero, &|_| {
            false
        });
        let x0 = owl.pos.x;
        assert!(!owl.damage(1.0,
            Some(Vec3::new(5.0, 0.0, 0.5)),
            false,
            None, &default_feedback()));
        owl.integrate(0.05, owl_def(), Vec3::ZERO, false, &floor_at_zero, &|_| {
            false
        });
        assert!(
            (owl.pos.x - x0).abs() < 1e-4,
            "non-attack damage should not shove the mob: {x0} -> {}",
            owl.pos.x
        );
    }

    #[test]
    fn every_hit_flashes_red_including_the_kill() {
        let mut owl = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
        owl.damage(
            1.0,
            Some(Vec3::new(5.0, 0.0, 0.5)),
            true,
            None,
            &default_feedback(),
        );
        assert!(owl.hurt_flash(1.0) > 0.0, "a non-lethal hit flashes red");

        // The killing blow flashes red too (so it looks like any other hit).
        let mut dead = Instance::new(Mob::Owl, Vec3::new(0.5, 0.0, 0.5), 0.0, 1);
        assert!(dead.damage(100.0,
            Some(Vec3::new(5.0, 0.0, 0.5)),
            true,
            None, &default_feedback()));
        assert!(
            dead.hurt_flash(1.0) > 0.0,
            "the kill flashes red like a normal hit"
        );
        assert!(
            dead.ragdoll_pose(0.5).is_none(),
            "ragdoll pose is None until a dead tick inits it"
        );
    }
}
