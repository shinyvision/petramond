use mod_sdk::*;

use crate::post_marker::PostRole;
use crate::skeleton::keys::{FACING_TAG, POST_TAG, RETURNING_TAG, ROLE_TAG};
use crate::skeleton::leash::decide;
use crate::skeleton::posts::role_tag;

const POST: [i32; 3] = [20, 64, 30];
const STAND: [i32; 3] = [20, 65, 30];

fn ctx(pos: [f64; 3], tags: Vec<(&str, MobTagValue)>) -> AiNodeCtx {
    AiNodeCtx {
        mob_id: 7,
        pos,
        cell: pos.map(|c| c.floor() as i32),
        yaw: 0.0,
        tick: 100,
        player_id: PlayerId(0),
        player_pos: [0.0; 3],
        nav_idle: true,
        in_fluid: None,
        target: None,
        attacker: None,
        player_held: None,
        player_foothold: None,
        tags: tags.into_iter().map(|(k, v)| (k.to_owned(), v)).collect(),
    }
}

fn post_tags(role: PostRole, returning: bool) -> Vec<(&'static str, MobTagValue)> {
    let mut tags = vec![
        (POST_TAG, POST.to_tag()),
        (ROLE_TAG, role_tag(role)),
        (FACING_TAG, MobTagValue::F64(1.5)),
    ];
    if returning {
        tags.push((RETURNING_TAG, MobTagValue::Bool(true)));
    }
    tags
}

fn returning_write(d: &AiNodeDecision) -> Option<Option<MobTagValue>> {
    d.tags
        .iter()
        .find(|w| w.key == RETURNING_TAG)
        .map(|w| w.value.clone())
}

#[test]
fn a_guard_without_a_post_is_left_alone() {
    assert_eq!(decide(&ctx([0.5, 65.0, 0.5], vec![])), None);
}

#[test]
fn an_idle_guard_that_strayed_walks_home_and_stops_there() {
    let far = ctx([30.5, 65.0, 30.5], post_tags(PostRole::Yard, false));
    let d = decide(&far).expect("a strayed guard heads home");
    assert_eq!(d.goal, Some(STAND), "the goal is where the feet stand");
    assert_eq!(returning_write(&d), Some(Some(MobTagValue::Bool(true))));

    let halfway = ctx([23.5, 65.0, 30.5], post_tags(PostRole::Yard, true));
    assert_eq!(
        decide(&halfway).and_then(|d| d.goal),
        Some(STAND),
        "inside the leash it keeps walking until it arrives"
    );

    let home =
        decide(&ctx([20.6, 65.0, 30.4], post_tags(PostRole::Yard, true))).expect("arrival is news");
    assert_eq!(home.goal, None);
    assert_eq!(
        returning_write(&home),
        Some(None),
        "arrived: no longer returning"
    );
    assert_eq!(
        home.facing,
        Some(1.5),
        "on its post it faces the post's way"
    );

    let wandering = ctx([23.5, 65.0, 30.5], post_tags(PostRole::Yard, false));
    assert_eq!(
        decide(&wandering),
        None,
        "a short wander is its own business"
    );
}

#[test]
fn a_guard_that_fell_from_its_post_goes_back_up() {
    let below = ctx([21.5, 61.0, 30.5], post_tags(PostRole::Yard, false));
    assert_eq!(decide(&below).and_then(|d| d.goal), Some(STAND));
}

#[test]
fn a_fighting_guard_is_not_called_home() {
    let mut fighting = ctx([35.5, 65.0, 30.5], post_tags(PostRole::Yard, true));
    fighting.target = Some(EntityRef::Player(PlayerId(1)));
    let d = decide(&fighting).expect("the return is called off");
    assert_eq!(d.goal, None);
    assert_eq!(returning_write(&d), Some(None));
}

#[test]
fn a_watch_guard_holds_its_post() {
    let on_post = decide(&ctx([20.5, 65.0, 30.5], post_tags(PostRole::Watch, false))).unwrap();
    assert!(
        on_post.claims.contains(DecisionChannel::Goal),
        "nothing walks it off"
    );
    assert_eq!(on_post.goal, None);
    assert_eq!(on_post.facing, Some(1.5));

    let nudged = decide(&ctx([22.5, 65.0, 30.5], post_tags(PostRole::Watch, false))).unwrap();
    assert_eq!(
        nudged.goal,
        Some(STAND),
        "even two blocks off, it goes back"
    );
}
