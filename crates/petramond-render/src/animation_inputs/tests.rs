use std::sync::Arc;

use glam::Vec3;
use petramond::player::RigId;
use petramond_world::animation::library::ClipLibrary;
use petramond_world::animation::test_rig::{clip, rig};
use petramond_world::animation::{Animator, Graph};
use petramond_world::item::{ItemRenderKind, ItemType};

use super::{item_facts, kind_fact, BodyDriver, BodyMotion, HandInputs};
use crate::views::AnimatorParamRow;
use crate::{AnimatorInputs, HeldItemFrame};

const RIG: RigId = RigId(5);
const DT: f32 = 1.0 / 60.0;
const EDGES: [&str; 3] = ["equip", "mine", "eat"];

impl BodyMotion for () {
    fn hurt(&self) -> f32 {
        0.0
    }
}

/// Each of the main hand's edge events plays a clip in a slot of its own,
/// so a playing slot says its event fired.
fn graph() -> Arc<Graph> {
    let m = rig();
    let mut lib = ClipLibrary::new();
    lib.insert(
        "a",
        clip(&m, 1.0, false, &[("leftArm", 0.0, Vec3::X * 10.0)]),
    );
    let graph = Graph::compile(
        r#"{ "params": { "main.mining": 0, "main.kind": "none", "main.item": "none", "main.tool": "none" },
             "events": ["main.equip", "main.mine", "main.eat"],
             "slots": ["equip", "mine", "eat"],
             "layers": [{ "slot": "equip" }, { "slot": "mine" }, { "slot": "eat" }],
             "rules": [
                { "on": "main.equip", "slot": "equip", "play": "a", "fade_in": 0, "fade_out": 0 },
                { "on": "main.mine", "slot": "mine", "play": "a", "fade_in": 0, "fade_out": 0 },
                { "on": "main.eat", "slot": "eat", "play": "a", "fade_in": 0, "fade_out": 0 }
             ] }"#,
        &m,
        lib,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    Arc::new(graph)
}

/// Publish one frame and answer which edges fired, then clear their slots.
fn publish(hand: &mut HandInputs, a: &mut Animator, frame: HeldItemFrame) -> [bool; 3] {
    hand.publish(a, &frame);
    a.update(DT);
    let fired = EDGES.map(|name| a.playing(a.graph().slot(name).unwrap()).is_some());
    for name in EDGES {
        a.stop(a.graph().slot(name).unwrap(), 0.0);
    }
    a.update(DT);
    fired
}

fn holding(item: Option<ItemType>) -> HeldItemFrame {
    HeldItemFrame {
        item,
        ..Default::default()
    }
}

/// Two registered items the hand draws differently.
fn block_and_sprite() -> (ItemType, ItemType) {
    let of = |want: fn(&ItemRenderKind) -> bool| {
        *ItemType::all()
            .iter()
            .find(|item| want(&item.render_kind()))
            .expect("the registry has one")
    };
    (
        of(|kind| matches!(kind, ItemRenderKind::BlockCube(_))),
        of(|kind| matches!(kind, ItemRenderKind::Sprite(_))),
    )
}

#[test]
fn equip_fires_when_the_held_stack_changes_but_not_on_a_first_frame_or_a_display_change() {
    let graph = graph();
    let (mut a, mut hand) = (
        Animator::new(Arc::clone(&graph), 1),
        HandInputs::new(&graph, "main"),
    );
    let (block, sprite) = block_and_sprite();
    let equip = |fired: [bool; 3]| fired[0];

    assert!(
        !equip(publish(&mut hand, &mut a, holding(Some(block)))),
        "the first frame is not a change"
    );
    assert!(!equip(publish(&mut hand, &mut a, holding(Some(block)))));
    assert!(
        equip(publish(&mut hand, &mut a, holding(Some(sprite)))),
        "a new stack"
    );
    let displayed = HeldItemFrame {
        display: Some(block),
        ..holding(Some(sprite))
    };
    assert!(
        !equip(publish(&mut hand, &mut a, displayed)),
        "a display changes only the look"
    );
    assert!(
        equip(publish(&mut hand, &mut a, holding(None))),
        "emptying the hand"
    );

    hand.reset();
    assert!(
        !equip(publish(&mut hand, &mut a, holding(Some(block)))),
        "the first frame after a reset"
    );
}

#[test]
fn mine_and_eat_fire_on_their_levels_rising_edge_only() {
    let graph = graph();
    let (mut a, mut hand) = (
        Animator::new(Arc::clone(&graph), 1),
        HandInputs::new(&graph, "main"),
    );
    let mining = |on| HeldItemFrame {
        mining: on,
        ..holding(None)
    };
    let eating = |on: bool| HeldItemFrame {
        eating: on.then_some(0.2),
        ..holding(None)
    };

    let mines: Vec<bool> = [false, true, true, false, true]
        .into_iter()
        .map(|on| publish(&mut hand, &mut a, mining(on))[1])
        .collect();
    assert_eq!(mines, [false, true, false, false, true]);
    let eats: Vec<bool> = [true, true, false, true]
        .into_iter()
        .map(|on| publish(&mut hand, &mut a, eating(on))[2])
        .collect();
    assert_eq!(eats, [true, false, false, true]);
}

#[test]
fn the_hold_kind_follows_the_drawn_item_and_the_facts_follow_the_held_stack() {
    let graph = graph();
    let (mut a, mut hand) = (
        Animator::new(Arc::clone(&graph), 1),
        HandInputs::new(&graph, "main"),
    );
    let (block, sprite) = block_and_sprite();
    let displayed = HeldItemFrame {
        display: Some(sprite),
        ..holding(Some(block))
    };
    publish(&mut hand, &mut a, displayed);
    assert_eq!(
        a.param(graph.param("main.kind").unwrap()),
        kind_fact(Some(sprite))
    );
    assert_eq!(
        a.param(graph.param("main.item").unwrap()),
        item_facts(Some(block))[0]
    );
}

#[test]
fn a_released_claim_on_an_item_fact_uncovers_the_engines_fact_the_next_frame() {
    let graph = graph();
    let kind = graph.param("main.kind").unwrap();
    let mut driver = BodyDriver::<()>::new(RIG, Arc::clone(&graph), 1, &[]);
    let (block, _) = block_and_sprite();
    let claimed = [AnimatorParamRow {
        rig: RIG,
        param: kind.index() as u16,
        value: -7.0,
    }];
    let mut frame = |params: &[AnimatorParamRow]| {
        driver.begin(Some(&()));
        driver.publish_hands(&[holding(Some(block)), holding(None)]);
        driver.claim(AnimatorInputs {
            params,
            ..Default::default()
        });
        driver.animator.update(DT);
        driver.animator.param(kind)
    };
    assert_eq!(frame(&claimed), -7.0);
    assert_eq!(
        frame(&[]),
        kind_fact(Some(block)),
        "the unchanged item still publishes"
    );
}
