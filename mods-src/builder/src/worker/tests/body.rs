use super::{at_work, working};
use crate::geometry::feet_of;
use crate::host::fake::rows::{AIR, STONE};
use crate::testing::HOME;
use crate::worker::body::{standing_cell, stands_at, unwedge};
use crate::worker::legs::{centre, hold_still};

#[test]
fn a_body_stands_in_the_cell_under_its_feet_or_the_one_it_rests_on() {
    let (session, _, _) = working(HOME);
    assert_eq!(standing_cell([4.5, 0.0, 4.5]), [4, 0, 4]);
    // Feet near the edge of a cell over a hole rest on the cell beside.
    session.world.set([4, -1, 4], AIR);
    assert!(!stands_at([4, 0, 4]) && stands_at([5, 0, 4]));
    assert_eq!(standing_cell([4.9, 0.0, 4.5]), [5, 0, 4]);
    assert_eq!(
        standing_cell([4.5, 0.0, 4.5]),
        [4, 0, 4],
        "square in its cell, the cell is its own"
    );
    // Feet on top of a block.
    session.world.set([6, 0, 6], STONE);
    assert_eq!(standing_cell([6.5, 1.0, 6.5]), [6, 1, 6]);
}

#[test]
fn a_body_off_the_centre_of_its_cell_is_nudged_back_and_held_still() {
    let (session, _, golem) = working([4, 0, 4]);
    let mut body = session.body(golem);
    body.pos = [4.2, 0.0, 4.5];
    centre(&body);
    let driven = || session.world.state().mobs[&golem].driven;
    assert_eq!(
        driven(),
        Some([2.0, 0.0, 0.0]),
        "its own legs, at their fastest"
    );
    hold_still(&body);
    assert_eq!(driven(), Some([0.0; 3]));
}

#[test]
fn a_body_wedged_under_a_block_is_set_down_on_free_ground_beside() {
    let (mut session, id, golem) = working([4, 0, 4]);
    let body = session.body(golem);
    assert!(!at_work(&mut session, id, |ctx, _, _| unwedge(ctx, &body)));
    assert_eq!(session.world.mob_pos(golem), Some(feet_of([4, 0, 4])));

    session.world.set([4, 1, 4], STONE);
    assert!(at_work(&mut session, id, |ctx, _, _| unwedge(ctx, &body)));
    assert_eq!(session.world.mob_pos(golem), Some(feet_of([3, 0, 4])));
}
