use super::{at_work, working};
use crate::geometry::feet_of;
use crate::host::fake::rows::{AIR, STONE};
use crate::testing::HOME;
use crate::worker::body::{standing_cell, stands_at, unwedge};

#[test]
fn a_body_stands_in_the_cell_under_its_feet_or_the_one_it_rests_on() {
    let (session, _, _) = working(HOME);
    assert_eq!(standing_cell([4.5, 0.0, 4.5]), [4, 0, 4]);
    session.world.set([4, -1, 4], AIR);
    assert!(!stands_at([4, 0, 4]) && stands_at([5, 0, 4]));
    assert_eq!(standing_cell([4.9, 0.0, 4.5]), [5, 0, 4]);
    assert_eq!(
        standing_cell([4.5, 0.0, 4.5]),
        [4, 0, 4],
        "square in its cell, the cell is its own"
    );
    session.world.set([6, 0, 6], STONE);
    assert_eq!(standing_cell([6.5, 1.0, 6.5]), [6, 1, 6]);
}

#[test]
fn a_body_wedged_under_a_block_is_set_down_on_free_ground_beside() {
    let (mut session, id, golem) = working([4, 0, 4]);
    let body = session.body(golem);
    assert!(!at_work(&mut session, id, |_, _, _| unwedge(&body)));
    assert_eq!(session.world.mob_pos(golem), Some(feet_of([4, 0, 4])));

    session.world.set([4, 1, 4], STONE);
    assert!(at_work(&mut session, id, |_, _, _| unwedge(&body)));
    assert_eq!(session.world.mob_pos(golem), Some(feet_of([3, 0, 4])));
}

#[test]
fn unwedge_checks_the_head_just_below_a_cell_boundary() {
    let (mut session, id, golem) = working([4, 0, 4]);
    let mut body = session.body(golem);
    body.pos[1] = 0.54999972227961;
    body.on_ground = false;
    session.world.state_mut().mobs.get_mut(&golem).unwrap().pos = body.pos;

    assert!(!at_work(&mut session, id, |_, _, _| unwedge(&body)));
    assert_eq!(session.world.mob_pos(golem), Some(body.pos));

    session.world.set([4, 1, 4], STONE);
    assert!(at_work(&mut session, id, |_, _, _| unwedge(&body)));
    assert_eq!(session.world.mob_pos(golem), Some(feet_of([3, 0, 4])));
}
