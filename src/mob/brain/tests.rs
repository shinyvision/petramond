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
/// A behavior that fills every channel a combat node would, and writes a tag.
struct Combatant;
impl AiBehavior for Combatant {
    fn tick(&mut self, _: &mut AiCtx) -> BehaviorOutput {
        BehaviorOutput {
            goal: Some(IVec3::ZERO),
            head_look: Some(HeadLook {
                yaw: 0.2,
                pitch: 0.0,
            }),
            target: Some(EntityRef::Mob(9)),
            attack: Some(AttackIntent {
                target: EntityRef::Mob(9),
                damage: 1.0,
                knockback: 0.0,
            }),
            tag_writes: vec![("test:ticked".into(), None)],
            ..Default::default()
        }
    }
}
/// A behavior that holds `claims` without filling anything.
struct Hold(ChannelClaims);
impl AiBehavior for Hold {
    fn tick(&mut self, _: &mut AiCtx) -> BehaviorOutput {
        BehaviorOutput {
            claims: self.0,
            ..Default::default()
        }
    }
}

#[test]
fn a_hold_settles_its_channels_empty_and_leaves_the_others_open() {
    let world = World::new(0, 1);
    let mut rng = MobRng::new(1);
    let holds = ChannelClaims::of(&[
        DecisionChannel::Goal,
        DecisionChannel::Target,
        DecisionChannel::Attack,
    ]);
    let mut brain = Brain::new()
        .with_boxed(PRIORITY_DAMAGE_RESPONSE, Box::new(Hold(holds)))
        .with_boxed(PRIORITY_ATTACK, Box::new(Combatant));
    let out = brain.decide(&mut ctx(&world, &mut rng));
    assert!(out.goal.is_none() && out.target.is_none() && out.attack.is_none());
    assert!(
        out.head_look.is_some(),
        "an unheld channel still composes in from below"
    );
    assert_eq!(out.claims, holds, "the settled decision reports the holds");
    assert_eq!(
        out.tag_writes.len(),
        1,
        "held-out nodes still advance their clocks and tag writes"
    );
}

#[test]
fn a_hold_below_a_filled_channel_changes_nothing() {
    let world = World::new(0, 1);
    let mut rng = MobRng::new(1);
    let mut brain = Brain::new()
        .with_boxed(100, Box::new(Goal(IVec3::new(9, 0, 0))))
        .with_boxed(50, Box::new(Hold(ChannelClaims::ALL)))
        .with_boxed(PRIORITY_WANDER, Box::new(Goal(IVec3::new(1, 0, 0))));
    let d = brain.decide(&mut ctx(&world, &mut rng));
    assert_eq!(d.goal, Some(IVec3::new(9, 0, 0)));
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

#[test]
fn filled_reports_exactly_the_channels_carrying_a_value() {
    let out = BehaviorOutput {
        goal: Some(IVec3::ZERO),
        animation: Some("swipe".into()),
        ..Default::default()
    };
    assert_eq!(
        out.filled(),
        ChannelClaims::of(&[DecisionChannel::Goal, DecisionChannel::Animation])
    );
    assert!(BehaviorOutput::default().filled().is_empty());
}
