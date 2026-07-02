//! Composable mob AI: a [`Brain`] is a priority-ordered set of [`AiBehavior`]s.
//!
//! Each game tick the brain asks every behavior for a [`BehaviorOutput`] and merges
//! them **per field by priority** (highest-priority behavior that sets a field wins
//! it): the navigation `goal`, the desired `head_look`, and any `idle_anim` to play.
//! So behaviors compose — wander supplies a goal, a head-look behavior supplies head
//! orientation, an idle-animation behavior supplies an anim — and a future
//! higher-priority behavior (flee, attack) just overrides the field(s) it cares
//! about by sitting above the others.
//!
//! Behaviors hold their own per-instance state, so — unlike the stateless `&'static`
//! block behaviors — they are owned per mob (`Box<dyn AiBehavior>`), built by the
//! species' `make_brain` fn.

use std::cmp::Reverse;

use crate::mathh::{IVec3, Vec3};
use crate::world::World;

use super::model_meta::IdleAnimMeta;
use super::{Mob, MobRng};

/// Priority of wander — the lowest, so any deliberate locomotion overrides it.
pub const PRIORITY_WANDER: u8 = 0;
/// Expressive (non-locomotion) behaviors. They set `head_look` / `idle_anim`, which
/// don't contend with `goal`, so their exact priority rarely matters — but giving
/// them a slot keeps the ordering explicit.
pub const PRIORITY_EXPRESSION: u8 = 10;
/// A desired head orientation **relative to the body** (radians): `yaw` swivels the
/// head left/right, `pitch` tilts it up/down. The renderer applies it to the model's
/// `head` bone (when the active animation isn't already moving the head).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct HeadLook {
    pub yaw: f32,
    pub pitch: f32,
}

/// Read-only mob state captured at the start of a mob tick for AI decisions that need
/// nearby companions without borrowing the live [`Mobs`](super::Mobs) container.
#[derive(Copy, Clone, Debug)]
pub struct AiMob {
    pub kind: Mob,
    pub pos: Vec3,
    pub active: bool,
}

/// Per-tick context a behavior reads to decide what the mob should do. Behaviors
/// mutate only their own state + the shared [`MobRng`]; the world is read-only.
pub struct AiCtx<'a> {
    /// Mob feet position (world space).
    pub pos: Vec3,
    /// Mob foothold cell (the voxel its feet occupy).
    pub cell: IVec3,
    /// Mob body facing (radians) — for resolving head-look yaw relative to the body.
    pub yaw: f32,
    /// Height of the mob's head above its feet (m) — for the look-at-player pitch.
    pub head_height: f32,
    /// Horizontal body radius from centre to side, for standable/pathing probes.
    pub half_width: f32,
    /// Read-only world, for sampling standable destinations / line-of-sight.
    pub world: &'a World,
    /// Player body-centre — for head-look (and future flee / attack).
    pub player_pos: Vec3,
    /// True when the navigator has no active path (arrived / gave up / untasked).
    /// Behaviors treat this as "the mob is idle".
    pub nav_idle: bool,
    /// True when the mob's body is in water. Behaviors react to it (e.g. idle
    /// animations don't play while swimming); the kinematics float the mob up.
    pub in_water: bool,
    /// The mob's vertical clearance in cells (its body height), for standable tests.
    pub head: i32,
    /// This species' `idle_*` animations (length + loop mode), so the idle-animation
    /// behavior only picks valid ones and plays a one-shot for its actual length.
    pub idle_anims: &'a [IdleAnimMeta],
    /// Index of this mob in [`mobs`](Self::mobs), so companion-aware behaviors can
    /// ignore the mob making the decision.
    pub mob_index: usize,
    /// Snapshot of live mobs at the start of this tick.
    pub mobs: &'a [AiMob],
    /// Deterministic per-mob RNG (no `rand` crate; reproducible).
    pub rng: &'a mut MobRng,
}

/// One behavior's contribution to a tick. Every field defaults to "no opinion"; the
/// brain keeps the highest-priority non-`None` value per field.
#[derive(Default, Clone, Copy, Debug, PartialEq)]
pub struct BehaviorOutput {
    /// A navigation destination this behavior wants the mob to head to.
    pub goal: Option<IVec3>,
    /// A desired head orientation (relative to the body).
    pub head_look: Option<HeadLook>,
    /// An `idle_*` animation index this behavior wants played.
    pub idle_anim: Option<u8>,
}

/// One composable unit of mob AI. Each tick it contributes a [`BehaviorOutput`].
pub trait AiBehavior {
    fn tick(&mut self, ctx: &mut AiCtx) -> BehaviorOutput;
}

/// A priority entry in a [`Brain`].
struct Entry {
    priority: u8,
    behavior: Box<dyn AiBehavior>,
}

/// A mob's full AI: its behaviors, evaluated highest-priority-first each tick.
#[derive(Default)]
pub struct Brain {
    entries: Vec<Entry>,
}

impl Brain {
    pub fn new() -> Self {
        Brain {
            entries: Vec::new(),
        }
    }

    /// Add a behavior at `priority` (higher wins), keeping the list sorted so
    /// [`decide`](Self::decide) scans it in order.
    pub fn with(mut self, priority: u8, behavior: impl AiBehavior + 'static) -> Self {
        self.entries.push(Entry {
            priority,
            behavior: Box::new(behavior),
        });
        // Highest priority first; stable so equal-priority behaviors keep insert order.
        self.entries.sort_by_key(|entry| Reverse(entry.priority));
        self
    }

    /// The merged decision for this tick: per field, the value from the
    /// highest-priority behavior that supplied one (`or` keeps the first `Some`,
    /// and behaviors are visited high→low priority).
    pub fn decide(&mut self, ctx: &mut AiCtx) -> BehaviorOutput {
        let mut decision = BehaviorOutput::default();
        for entry in &mut self.entries {
            let out = entry.behavior.tick(ctx);
            decision.goal = decision.goal.or(out.goal);
            decision.head_look = decision.head_look.or(out.head_look);
            decision.idle_anim = decision.idle_anim.or(out.idle_anim);
        }
        decision
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A behavior that always wants a fixed goal.
    struct Goal(IVec3);
    impl AiBehavior for Goal {
        fn tick(&mut self, _ctx: &mut AiCtx) -> BehaviorOutput {
            BehaviorOutput {
                goal: Some(self.0),
                ..Default::default()
            }
        }
    }
    /// A behavior that only ever sets a head-look (never a goal).
    struct Look(HeadLook);
    impl AiBehavior for Look {
        fn tick(&mut self, _ctx: &mut AiCtx) -> BehaviorOutput {
            BehaviorOutput {
                head_look: Some(self.0),
                ..Default::default()
            }
        }
    }
    /// A behavior that yields entirely.
    struct Yield;
    impl AiBehavior for Yield {
        fn tick(&mut self, _ctx: &mut AiCtx) -> BehaviorOutput {
            BehaviorOutput::default()
        }
    }

    fn ctx<'a>(world: &'a World, rng: &'a mut MobRng) -> AiCtx<'a> {
        AiCtx {
            pos: Vec3::ZERO,
            cell: IVec3::ZERO,
            yaw: 0.0,
            head_height: 0.7,
            half_width: 0.25,
            world,
            player_pos: Vec3::ZERO,
            nav_idle: true,
            in_water: false,
            head: 1,
            idle_anims: &[],
            mob_index: 0,
            mobs: &[],
            rng,
        }
    }

    #[test]
    fn higher_priority_goal_wins_but_fields_compose() {
        let world = World::new(0, 1);
        let mut rng = MobRng::new(1);
        let look = HeadLook {
            yaw: 0.5,
            pitch: 0.1,
        };
        // Wander (low) wants one goal, a high-priority behavior wants another + a
        // head-look. The goal comes from the high-priority one; the head-look (set by
        // nobody else) still composes in.
        let mut brain = Brain::new()
            .with(PRIORITY_WANDER, Goal(IVec3::new(1, 0, 0)))
            .with(PRIORITY_EXPRESSION, Look(look))
            .with(100, Goal(IVec3::new(9, 0, 0)));
        let d = brain.decide(&mut ctx(&world, &mut rng));
        assert_eq!(
            d.goal,
            Some(IVec3::new(9, 0, 0)),
            "highest-priority goal wins"
        );
        assert_eq!(
            d.head_look,
            Some(look),
            "an orthogonal field still composes in"
        );
    }

    #[test]
    fn yielding_behaviors_leave_fields_none() {
        let world = World::new(0, 1);
        let mut rng = MobRng::new(1);
        let mut brain = Brain::new().with(PRIORITY_WANDER, Yield);
        let d = brain.decide(&mut ctx(&world, &mut rng));
        assert_eq!(d, BehaviorOutput::default());
    }

    #[test]
    fn lower_priority_fills_a_field_a_higher_one_left_unset() {
        let world = World::new(0, 1);
        let mut rng = MobRng::new(1);
        // The high-priority behavior only sets head_look; the low one supplies the goal.
        let mut brain = Brain::new()
            .with(PRIORITY_WANDER, Goal(IVec3::new(2, 0, 0)))
            .with(
                100,
                Look(HeadLook {
                    yaw: 0.0,
                    pitch: 0.0,
                }),
            );
        let d = brain.decide(&mut ctx(&world, &mut rng));
        assert_eq!(
            d.goal,
            Some(IVec3::new(2, 0, 0)),
            "goal falls through to wander"
        );
        assert!(d.head_look.is_some());
    }
}
