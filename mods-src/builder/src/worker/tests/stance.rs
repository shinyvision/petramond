use super::{at_work, working};
use crate::host::fake::record;
use crate::host::fake::rows::{AIR, STONE};
use crate::testing::HOME;
use crate::worker::route::Hubs;
use crate::worker::sight::Work;
use crate::worker::stance::{self, Search};

fn lay(pos: [i32; 3]) -> Work {
    Work::Click {
        pos,
        record: Some(record("petramond:stone")),
    }
}

fn found(search: Search<[i32; 3]>) -> Option<[i32; 3]> {
    match search {
        Search::Found(stance) => Some(stance),
        _ => None,
    }
}

#[test]
fn the_stance_is_the_nearest_walk_that_sees_the_work() {
    let (mut session, id, golem) = working([0, 0, -6]);
    let body = session.body(golem);
    let target = [5, 0, -6];
    let stance = at_work(&mut session, id, |ctx, _, job| {
        let hubs = Hubs::new(HOME, &job.crew.trail);
        found(stance::find(
            ctx,
            &body,
            hubs,
            &[target],
            &lay(target),
            false,
            |_| true,
        ))
    });
    assert_eq!(stance, Some([1, 0, -6]), "one step on, and in reach");

    let stance = at_work(&mut session, id, |ctx, _, job| {
        let hubs = Hubs::new(HOME, &job.crew.trail);
        found(stance::find(
            ctx,
            &body,
            hubs,
            &[target],
            &lay(target),
            false,
            |s| s != [1, 0, -6],
        ))
    });
    assert!(stance.is_some_and(|s| s != [1, 0, -6]));
}

#[test]
fn work_walled_in_is_unseen_and_work_in_the_air_has_nowhere_to_stand() {
    let (mut session, id, golem) = working([0, 0, -6]);
    let target = [5, 0, -6];
    session.world.fill([4, 0, -7], [6, 1, -5], STONE);
    session.world.set(target, AIR);
    let body = session.body(golem);
    at_work(&mut session, id, |ctx, _, job| {
        let hubs = Hubs::new(HOME, &job.crew.trail);
        let walled = stance::find(ctx, &body, hubs, &[target], &lay(target), false, |_| true);
        assert!(matches!(walled, Search::Unseen));
        let high = [5, 12, -6];
        let aloft = stance::find(ctx, &body, hubs, &[high], &lay(high), false, |_| true);
        assert!(matches!(aloft, Search::None));
    });
}

#[test]
fn a_stance_search_with_no_route_budget_asks_again() {
    let (mut session, id, golem) = working([0, 0, -6]);
    session.world.route_budget(Some(0));
    let body = session.body(golem);
    let target = [5, 0, -6];
    at_work(&mut session, id, |ctx, _, job| {
        let hubs = Hubs::new(HOME, &job.crew.trail);
        let busy = stance::find(ctx, &body, hubs, &[target], &lay(target), false, |_| true);
        assert!(matches!(busy, Search::Busy));
    });
}
