use crate::skeleton::posts::{
    may_fill, Marker, Posts, Scan, RESPAWN, RETRY_WINDOW, SETTLE, SIGHT, SPAWN_MAX, SPAWN_MIN,
};

const SECTION: [i32; 3] = [2, 4, -1];
const POST_A: [i32; 3] = [35, 70, -10];
const POST_B: [i32; 3] = [40, 71, -3];

fn yard() -> Marker {
    Marker { role: 0, yaw: None }
}

fn found(cells: &[[i32; 3]]) -> Scan {
    Scan::Found(cells.iter().map(|c| (*c, yard())).collect())
}

fn due_cells(posts: &Posts, now: u64) -> Vec<[i32; 3]> {
    posts.due(now).into_iter().map(|(c, _)| c).collect()
}

#[test]
fn markers_decode_role_and_yaw() {
    let east = Marker::decode(&[1, 192]).expect("two bytes");
    assert!(east.watch());
    assert!((east.yaw.unwrap() - 0.75 * std::f32::consts::TAU).abs() < 1e-5);
    assert_eq!(
        Marker::decode(&[0, 0xFF]).unwrap().yaw,
        None,
        "no preference"
    );
    assert_eq!(Marker::decode(&[0]), None);
    assert_eq!(Marker::decode(&[0, 1, 2]), None);
}

#[test]
fn a_section_that_was_not_ready_is_asked_again_for_a_while() {
    let mut posts = Posts::default();
    posts.queue(SECTION, 100);
    posts.settle(SECTION, Scan::NotReady, 101);
    assert_eq!(posts.pending(), [SECTION], "asked again next tick");
    posts.settle(SECTION, Scan::NotReady, 100 + RETRY_WINDOW + 1);
    assert!(
        posts.pending().is_empty(),
        "a section that never settles is let go"
    );
    posts.queue(SECTION, 500);
    posts.settle(SECTION, found(&[POST_A]), 502);
    assert!(posts.pending().is_empty());
    assert!(posts.get(&POST_A).is_some());
}

#[test]
fn a_new_post_fills_once_its_section_settles() {
    let mut posts = Posts::default();
    posts.settle(SECTION, found(&[POST_A]), 1000);
    assert!(
        due_cells(&posts, 1000 + SETTLE - 1).is_empty(),
        "its guard may still be loading"
    );
    assert_eq!(
        due_cells(&posts, 1000 + SETTLE),
        [POST_A],
        "never seen manned: fill now"
    );
    posts.set_occupied(POST_A);
    assert!(
        due_cells(&posts, 5000).is_empty(),
        "a manned post is not due"
    );
    posts.census(|_| false);
    assert_eq!(
        due_cells(&posts, 5000),
        [POST_A],
        "a guard that despawned is not waited for"
    );
}

#[test]
fn a_killed_guard_is_replaced_only_after_the_respawn_delay() {
    let mut posts = Posts::default();
    posts.settle(SECTION, found(&[POST_A, POST_B]), 0);
    posts.census(|_| true);
    posts.record_death(POST_A, 2000);
    posts.census(|cell| *cell == POST_B);
    assert!(due_cells(&posts, 2000 + RESPAWN - 1).is_empty());
    assert_eq!(due_cells(&posts, 2000 + RESPAWN), [POST_A]);
}

#[test]
fn a_rescanned_section_replaces_its_posts_and_keeps_what_it_knew_of_the_rest() {
    let mut posts = Posts::default();
    posts.settle(SECTION, found(&[POST_A, POST_B]), 0);
    posts.record_death(POST_A, 100);
    posts.settle(SECTION, found(&[POST_A]), 200);
    assert!(
        posts.get(&POST_B).is_none(),
        "the reloaded section no longer marks it"
    );
    assert!(
        due_cells(&posts, 200 + SETTLE).is_empty(),
        "the death outlives the reload"
    );
    assert_eq!(due_cells(&posts, 100 + RESPAWN), [POST_A]);
    posts.settle(SECTION, found(&[]), 300);
    assert!(
        posts.get(&POST_A).is_none(),
        "an emptied section forgets its posts"
    );
}

#[test]
fn a_post_fills_only_while_a_player_is_near_and_nobody_sees_it() {
    let post = [0.0, 70.0, 0.0];
    let at = |d: f64| [d, 70.0, 0.0];
    let blind = |_: [f64; 3], _: [f64; 3]| false;
    let sighted = |_: [f64; 3], _: [f64; 3]| true;
    assert!(!may_fill(post, &[], blind), "nobody around");
    assert!(
        !may_fill(post, &[at(SPAWN_MAX + 1.0)], blind),
        "too far to matter"
    );
    assert!(may_fill(post, &[at(SPAWN_MIN + 1.0)], blind));
    assert!(
        !may_fill(post, &[at(SPAWN_MIN - 1.0)], blind),
        "someone right there"
    );
    assert!(
        !may_fill(post, &[at(SIGHT - 1.0)], sighted),
        "in plain view"
    );
    assert!(
        may_fill(post, &[at(SIGHT + 1.0)], sighted),
        "seen, but from too far to notice"
    );
    assert!(
        !may_fill(post, &[at(SPAWN_MAX + 20.0), at(SPAWN_MIN - 1.0)], blind),
        "any player too close refuses"
    );
}

#[test]
fn a_post_is_fresh_until_a_guard_has_held_it_or_died_on_it() {
    let mut posts = Posts::default();
    posts.queue(SECTION, 0);
    posts.settle(SECTION, found(&[POST_A, POST_B]), 0);
    assert!(posts.fresh(&POST_A) && posts.fresh(&POST_B));
    posts.set_occupied(POST_A);
    posts.record_death(POST_B, 10);
    assert!(!posts.fresh(&POST_A), "a guard stood there");
    assert!(!posts.fresh(&POST_B), "a guard died there");
    assert!(
        !posts.fresh(&[0, 0, 0]),
        "an unknown post is nobody's first fill"
    );
}
