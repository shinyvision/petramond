use super::{at_work, working};
use crate::geometry::manhattan;
use crate::host::fake::rows::{AIR, STONE};
use crate::host::fake::Fake;
use crate::host::prelude::*;
use crate::testing::HOME;
use crate::worker::route::{self, Hubs, Trail};
use crate::worker::tuning::route::{LEG, MEMORY};

/// A cell walled in on every side, two blocks high: no body gets in or out.
const PEN: [i32; 3] = [5, 0, -6];

fn pen(world: &Fake) {
    world.fill([4, 0, -7], [6, 1, -5], STONE);
    world.set(PEN, AIR);
    world.set([PEN[0], 1, PEN[2]], AIR);
}

#[test]
fn a_route_answer_stands_for_its_memory() {
    let (mut session, id, _) = working(HOME);
    at_work(&mut session, id, |ctx, _, _| {
        assert_eq!(route::probe(ctx, [0, 0, -6], PEN, Vec::new()), Some(Route::Open));
    });
    pen(&session.world);
    at_work(&mut session, id, |ctx, _, _| {
        assert_eq!(
            route::probe(ctx, [0, 0, -6], PEN, Vec::new()),
            Some(Route::Open),
            "remembered"
        );
        assert_eq!(route::remembered(ctx, [0, 0, -6], PEN), Some(Route::Open));
        ctx.now += MEMORY;
        assert_eq!(route::remembered(ctx, [0, 0, -6], PEN), None);
        assert_eq!(route::probe(ctx, [0, 0, -6], PEN, Vec::new()), Some(Route::Closed));
        // Cells treated as built are another question.
        assert_eq!(
            route::probe(ctx, [0, 0, -6], [2, 0, -6], vec![[1, 0, -6]]),
            Some(Route::Open),
            "round them"
        );
    });
}

#[test]
fn a_spent_route_budget_is_no_answer_for_the_rest_of_the_tick() {
    let (mut session, id, _) = working(HOME);
    session.world.route_budget(Some(0));
    at_work(&mut session, id, |ctx, _, _| {
        assert_eq!(route::probe(ctx, [0, 0, -6], [3, 0, -6], Vec::new()), None);
        assert_eq!(*ctx.probe_nodes, 0);
        assert!(route::region(ctx, [0, 0, -6], false, &[]).is_none());
    });
    session.world.route_budget(Some(1));
    at_work(&mut session, id, |ctx, _, _| {
        assert!(matches!(route::region(ctx, [0, 0, -6], false, &[]), Some(Some(_))));
        assert!(
            matches!(route::region(ctx, [0, 0, -6], false, &[]), Some(Some(_))),
            "a flood is remembered like a route"
        );
        assert_eq!(route::probe(ctx, [0, 0, -6], [3, 0, -6], Vec::new()), None);
        assert!(route::region(ctx, [1, 0, -6], false, &[]).is_none());
    });
}

#[test]
fn a_flood_counts_the_moves_to_every_foothold_of_the_site() {
    let (mut session, id, _) = working(HOME);
    pen(&session.world);
    session.world.set([0, 0, -4], STONE);
    at_work(&mut session, id, |ctx, _, _| {
        let region = route::region(ctx, [0, 0, -6], false, &[])
            .flatten()
            .expect("a flood of the site");
        assert_eq!(region.moves([0, 0, -6]), Some(0));
        assert_eq!(region.moves([3, 0, -6]), Some(3));
        assert_eq!(region.moves([0, 1, -4]), Some(2), "a step up onto the block");
        assert_eq!(region.moves([0, 0, -4]), None, "inside it");
        assert!(!region.contains(PEN));
        assert!(region.contains(HOME));
        assert_eq!(region.cells().next(), Some([0, 0, -6]), "nearest first");
        assert!(
            route::region(ctx, [40, 0, 0], false, &[]).unwrap().is_none(),
            "off the site"
        );
        assert_eq!(route::moves_or_guess(ctx, [0, 0, -6], [3, 0, -6]), 3);
        assert_eq!(
            route::moves_or_guess(ctx, [0, 0, -6], PEN),
            2 * manhattan(PEN, [0, 0, -6]),
            "unwalked: twice as the crow flies"
        );
    });
}

#[test]
fn a_round_trip_needs_a_way_out_and_a_way_back() {
    let (mut session, id, _) = working(HOME);
    pen(&session.world);
    at_work(&mut session, id, |ctx, _, _| {
        let trail = Trail::default();
        let hubs = Hubs::new(HOME, &trail);
        assert_eq!(route::out(ctx, hubs, HOME, &[]), Some(Route::Open));
        assert_eq!(route::out(ctx, hubs, PEN, &[]), Some(Route::Closed));
        assert_eq!(route::round_trip(ctx, hubs, [3, 0, -6]), Some(true));
        assert!(!route::failed_recently(ctx, [3, 0, -6], HOME));
        assert_eq!(route::round_trip(ctx, hubs, PEN), Some(false));
        assert!(route::failed_recently(ctx, PEN, HOME));
        assert_eq!(route::round_trip(ctx, hubs, HOME), Some(true));
    });
}

#[test]
fn a_long_walk_goes_in_legs_toward_the_goal() {
    let (mut session, id, golem) = working([-9, 0, -9]);
    pen(&session.world);
    let body = session.body(golem);
    at_work(&mut session, id, |ctx, _, job| {
        let hubs = Hubs::new(HOME, &job.crew.trail);
        assert_eq!(
            route::leg(ctx, hubs, &body, [-7, 0, -9]),
            Some(Some([-7, 0, -9])),
            "near: straight there"
        );
        let goal = [9, 0, 9];
        let first = route::leg(ctx, hubs, &body, goal)
            .flatten()
            .expect("a leg toward it");
        assert_ne!(first, goal);
        assert!(manhattan(first, body.cell) <= LEG);
        assert!(manhattan(first, goal) < manhattan(body.cell, goal));
        assert_eq!(route::leg(ctx, hubs, &body, body.cell), Some(Some(body.cell)));
        assert_eq!(route::leg(ctx, hubs, &body, PEN), Some(None), "no way there");
    });
}
