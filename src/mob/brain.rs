//! Composable mob AI: a [`Brain`] is a priority-ordered set of [`AiBehavior`]s.
//!
//! Each game tick the brain asks every behavior for a [`BehaviorOutput`] and merges
//! them **per field by priority** (highest-priority behavior that sets a field wins
//! it): the navigation `goal`, the desired `head_look`, any `idle_anim` to play, and
//! any melee `attack` to land. So behaviors compose — wander supplies a goal, a
//! head-look behavior supplies head orientation, chase overrides the goal while a
//! player is near — each just owning the field(s) it cares about at its priority.
//!
//! Behaviors hold their own per-instance state, so — unlike the stateless `&'static`
//! block behaviors — they are owned per mob (`Box<dyn AiBehavior>`), built per spawn
//! from the species' data brain rows (see `mob::build_brain` and `mob::behavior`).

use std::cmp::Reverse;
use std::collections::BTreeMap;

use crate::mathh::{IVec3, Vec3};
use crate::world::World;

use super::model_meta::IdleAnimMeta;
use super::noise::Noise;
use super::{EntityRef, Mob, MobRng, PlayerAnchor};

/// Priority of wander — the lowest, so any deliberate locomotion overrides it.
pub const PRIORITY_WANDER: u8 = 0;
/// Expressive (non-locomotion) behaviors. They set `head_look` / `idle_anim`, which
/// don't contend with `goal`, so their exact priority rarely matters — but giving
/// them a slot keeps the ordering explicit.
pub const PRIORITY_EXPRESSION: u8 = 10;
/// Chase locomotion (`chase_player`, `chase_sound`) — above wander, so hunting
/// overrides roaming.
pub const PRIORITY_CHASE: u8 = 20;
/// Contact aggression (`chase_contact`) — above ordinary chases: something
/// touching the mob's body beats whatever it was hunting at a distance.
pub const PRIORITY_CONTACT: u8 = 22;
/// Retaliation (`retaliate`) — above contact: a mob that was actually hit
/// turns on its attacker before answering a mere bump.
pub const PRIORITY_RETALIATE: u8 = 25;
/// Attack behaviors (`melee_attack`) — above chase; they own the `attack` field (which
/// nothing else contends for), and the explicit slot keeps the ordering readable.
pub const PRIORITY_ATTACK: u8 = 30;
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
#[derive(Clone, Debug)]
pub struct AiMob {
    /// Stable session id — what noise sources, targets, and attacker memory
    /// name, so a lock survives `swap_remove` renumbering across ticks.
    pub id: u64,
    pub kind: Mob,
    /// Feet position (like `Instance::pos`).
    pub pos: Vec3,
    pub active: bool,
    /// Engine- and mod-owned tags attached to this mob instance, cloned from
    /// [`Instance::tags`](super::Instance::tags) for the snapshot. Behaviors can
    /// query any tag generically; the engine reserves the `petramond:` namespace.
    pub tags: BTreeMap<String, super::MobTagValue>,
}

impl AiMob {
    /// Whether this mob carries `key` with a truthy `Bool` value.
    pub fn bool_tag(&self, key: &str) -> bool {
        matches!(self.tags.get(key), Some(super::MobTagValue::Bool(true)))
    }

    /// Whether this mob is confined (captive / penned). Behaviors that rely on
    /// a mob's freedom of movement (e.g., herd cohesion) should ignore confined
    /// companions.
    pub fn confined(&self) -> bool {
        self.bool_tag("petramond:confined")
    }
}

/// The tick-wide, read-only inputs every mob's AI shares this tick — built once
/// by the manager and threaded to each instance. New world-level perception
/// channels extend this struct, not the instance tick signature.
pub struct TickInputs<'a> {
    pub world: &'a World,
    /// Every connected player's anchor.
    pub players: &'a [PlayerAnchor],
    /// The gameplay noises audible this tick.
    pub noises: &'a [Noise],
    /// Snapshot of live mobs at the start of this tick.
    pub mobs: &'a [AiMob],
    /// Rigid movement obstacles for this mob. Soft bodies receive the complete
    /// start-of-tick solid snapshot; a moving solid receives only exact peer
    /// supports, with every other solid handled by the simultaneous solver.
    pub solid: &'a [crate::collision::DynBox],
    /// Complete start-of-tick solid snapshot used only to clamp the mandatory
    /// shallow-foot healing lift. Moving solids otherwise receive just their
    /// exact supports here and meet all other peers in the simultaneous solve.
    pub solid_heal: &'a [crate::collision::DynBox],
}

/// Per-tick context a behavior reads to decide what the mob should do. Behaviors
/// mutate only their own state + the shared [`MobRng`]; the world is read-only.
pub struct AiCtx<'a> {
    /// The mob's stable id (spawn-counter identity) — scripted (WASM) nodes
    /// key per-mob guest state off it.
    pub mob_id: u64,
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
    /// The NEAREST player's session id — pairs with [`player_pos`](Self::player_pos);
    /// what a player-anchored behavior (chase, melee fallback) targets.
    pub player_id: crate::server::player::PlayerId,
    /// Player body-centre — for head-look (and future flee / attack).
    pub player_pos: Vec3,
    /// Whether that player is sneaking — sneaking shrinks hostile detection
    /// (see `chase_player`'s `sneak_radius_penalty`).
    pub player_sneaking: bool,
    /// That player's selected (held) item — the hand fact lure/beg behaviors
    /// read. `None` for an empty hand or a spectator.
    pub player_held: Option<crate::item::ItemType>,
    /// EVERY connected player's anchor, for behaviors that track a SPECIFIC
    /// player (a heard target, an attacker) rather than the nearest one.
    pub players: &'a [PlayerAnchor],
    /// The gameplay noises audible this tick (see [`super::noise`]) — the
    /// perception input for hearing-based behaviors. Radius/memory policy
    /// lives on the listening node.
    pub noises: &'a [Noise],
    /// The entities whose bodies overlapped THIS mob on the previous tick —
    /// the TOUCH perception channel, recorded by the manager's push pass
    /// (which already finds every overlapping pair). Sneaking silences
    /// footsteps, but nothing silences a body pressed against yours.
    pub contacts: &'a [EntityRef],
    /// The target the whole brain settled on LAST tick (the merged
    /// [`BehaviorOutput::target`]) — how an attack node strikes what a
    /// perception node locked, without in-pass ordering coupling.
    pub target: Option<EntityRef>,
    /// Who last damaged this mob, and how many ticks ago — the retaliation
    /// input. Recorded by the damage pipeline; `None` until first hit.
    pub attacker: Option<(EntityRef, u32)>,
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

impl AiCtx<'_> {
    /// Whether `who` still exists as a targetable entity this tick (a
    /// connected player, or a live mob in the snapshot).
    pub fn entity_alive(&self, who: EntityRef) -> bool {
        match who {
            EntityRef::Player(pid) => self.players.iter().any(|a| a.id == pid),
            EntityRef::Mob(id) => self.mobs.iter().any(|m| m.id == id && m.active),
        }
    }

    /// `who`'s live body-centre position, or `None` when it is gone/dead.
    /// (Player anchors are body centres already; mob snapshots carry feet.)
    pub fn entity_pos(&self, who: EntityRef) -> Option<Vec3> {
        match who {
            EntityRef::Player(pid) => self.players.iter().find(|a| a.id == pid).map(|a| a.pos),
            EntityRef::Mob(id) => self
                .mobs
                .iter()
                .find(|m| m.id == id && m.active)
                .map(|m| m.pos + Vec3::new(0.0, super::def(m.kind).size.height * 0.5, 0.0)),
        }
    }
}

/// A melee strike a behavior wants to land THIS tick. The instance latches it,
/// the manager drains it into a [`MobAttack`](super::MobAttack) (deriving the
/// knockback direction from the live attacker→target positions), and `Game`
/// applies it through the matching damage pipeline: engine immunity plus
/// `player_damage_pre` for a player target (either rejection drops damage AND
/// knockback together), or the mob damage pipeline for a mob target (global
/// immunity, `mob_damage_pre`, feedback, loot, ragdoll — mob-vs-mob combat is
/// the same funnel as every other mob hit).
/// Cooldown state lives on the emitting node, not here.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct AttackIntent {
    /// Who the strike lands on.
    pub target: EntityRef,
    /// Damage in half-heart points (rounded when applied to a player; mobs
    /// keep the fraction).
    pub damage: f32,
    /// Horizontal knockback speed (m/s) imparted away from the mob. Applies to
    /// player targets; a mob target takes its row's own `petramond:knockback`
    /// feedback component instead.
    pub knockback: f32,
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
    /// A melee strike this behavior wants landed this tick.
    pub attack: Option<AttackIntent>,
    /// The entity this behavior is engaged on. The merged value is latched by
    /// the instance and fed back as next tick's [`AiCtx::target`], so attack
    /// nodes strike what the winning perception/chase node locked.
    pub target: Option<EntityRef>,
}

/// One composable unit of mob AI. Each tick it contributes a [`BehaviorOutput`].
/// `Send` because mobs ride the `World`, which lives on the server
/// thread — behaviors are plain state machines; the scripted
/// (WASM) node holds only its registry key.
pub trait AiBehavior: Send {
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

    /// Add a (boxed — the AI-node factories return trait objects) behavior at
    /// `priority` (higher wins), keeping the list sorted so
    /// [`decide`](Self::decide) scans it in order.
    pub fn with_boxed(mut self, priority: u8, behavior: Box<dyn AiBehavior>) -> Self {
        self.entries.push(Entry { priority, behavior });
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
            decision.attack = decision.attack.or(out.attack);
            decision.target = decision.target.or(out.target);
        }
        decision
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mob::behavior::test_support::ctx;

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
            .with_boxed(PRIORITY_WANDER, Box::new(Goal(IVec3::new(1, 0, 0))))
            .with_boxed(PRIORITY_EXPRESSION, Box::new(Look(look)))
            .with_boxed(100, Box::new(Goal(IVec3::new(9, 0, 0))));
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
        let mut brain = Brain::new().with_boxed(PRIORITY_WANDER, Box::new(Yield));
        let d = brain.decide(&mut ctx(&world, &mut rng));
        assert_eq!(d, BehaviorOutput::default());
    }

    #[test]
    fn lower_priority_fills_a_field_a_higher_one_left_unset() {
        let world = World::new(0, 1);
        let mut rng = MobRng::new(1);
        // The high-priority behavior only sets head_look; the low one supplies the goal.
        let mut brain = Brain::new()
            .with_boxed(PRIORITY_WANDER, Box::new(Goal(IVec3::new(2, 0, 0))))
            .with_boxed(
                100,
                Box::new(Look(HeadLook {
                    yaw: 0.0,
                    pitch: 0.0,
                })),
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
