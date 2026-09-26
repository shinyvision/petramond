use super::*;

fn ctx(tags: &[(&str, MobTagValue)]) -> AiNodeCtx {
    AiNodeCtx {
        mob_id: 1,
        pos: [0.5, 0.0, 0.5],
        cell: [0, 0, 0],
        yaw: 0.0,
        tick: 1,
        player_id: PlayerId(0),
        player_pos: [0.0; 3],
        nav_idle: true,
        in_fluid: None,
        target: None,
        attacker: None,
        player_held: None,
        player_foothold: None,
        tags: tags
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect(),
    }
}

fn project() -> (&'static str, MobTagValue) {
    (PROJECT_TAG, MobTagValue::I64(3))
}

fn look(point: [f64; 3]) -> Option<[f32; 2]> {
    look_tagged(point.to_tag())
}

fn look_tagged(tag: MobTagValue) -> Option<[f32; 2]> {
    let decision = decide(&ctx(&[project(), (LOOK_TAG, tag)])).expect("a golem at work is steered");
    assert_eq!(
        decision.claims.contains(DecisionChannel::HeadLook),
        decision.head_look.is_some(),
        "the head is claimed only while it looks at something"
    );
    decision.head_look
}

#[test]
fn only_a_golem_bound_to_a_project_is_steered() {
    assert!(decide(&ctx(&[])).is_none());
    assert!(decide(&ctx(&[(GOAL_TAG, [1, 0, 1].to_tag())])).is_none());
}

#[test]
fn a_golem_walks_to_its_goal_unless_held_and_stays_put_without_one() {
    let goal = (GOAL_TAG, [4, 0, -2].to_tag());
    let walking = decide(&ctx(&[project(), goal.clone()])).unwrap();
    assert_eq!(walking.goal, Some([4, 0, -2]));
    assert!(walking.claims.contains(DecisionChannel::Goal));
    assert!(walking.claims.contains(DecisionChannel::Facing));
    assert!(!walking.claims.contains(DecisionChannel::HeadLook));
    assert_eq!(walking.facing, None);

    let held = decide(&ctx(&[
        project(),
        goal,
        (HOLD_TAG, MobTagValue::Bool(true)),
    ]))
    .unwrap();
    assert_eq!(held.goal, None, "held: the goal is not walked to");
    let idle = decide(&ctx(&[project()])).unwrap();
    assert_eq!(idle.goal, Some([0, 0, 0]), "no goal: where it stands");
    let turned = decide(&ctx(&[project(), (FACE_TAG, MobTagValue::F64(1.25))])).unwrap();
    assert_eq!(turned.facing, Some(1.25));
}

#[test]
fn the_head_turns_as_far_as_the_neck_goes_toward_what_it_looks_at() {
    let [yaw, pitch] = look([0.5, 1.3, -2.5]).expect("straight ahead");
    assert!(yaw.abs() < 1e-6 && pitch.abs() < 1e-6);
    let [yaw, _] = look([0.5, 1.3, 3.5]).expect("behind");
    assert_eq!(
        yaw.abs(),
        NECK_YAW,
        "the body comes round the rest of the way"
    );
    let [yaw, pitch] = look([0.5, -10.0, 0.5]).expect("underfoot");
    assert_eq!((yaw, pitch), (0.0, -NECK_PITCH));
    assert_eq!(
        look_tagged(MobTagValue::Str("0.5 1.3 -2.5".into())),
        None,
        "text is no point"
    );
}
