use super::{at_work, working};
use crate::content::{BLUEPRINT, EARTH_BURST};
use crate::host::fake::Deed;
use crate::host::prelude::*;
use crate::project::Phase;
use crate::testing::{HOME, TABLE_AT};
use crate::worker::lifecycle::{burrow, emerge, relocate, EMERGE};
use crate::worker::tuning::body::{BURROW_DEPTH, BURROW_TICKS, EMERGE_TICKS};
use crate::worker::Step;

#[test]
fn a_golem_rises_out_of_the_ground_and_takes_its_plans_from_the_table() {
    let (mut session, id, golem) = working(HOME);
    session.builder.projects.update(id, |p| p.summon(HOME));
    session.world.put(ContainerAddress::Mob(golem), 0, None);
    let body = session.body(golem);
    at_work(&mut session, id, |_, projects, job| {
        job.crew.step = Step::Emerge { t: 0 };
        emerge(projects, job, &body);
        assert_eq!(job.crew.step, Step::Emerge { t: 1 });
    });
    let deeds = session.world.deeds();
    assert!(deeds.contains(&Deed::Burst(EARTH_BURST.into())));
    assert!(deeds.contains(&Deed::Sound("petramond:dirt_break".into())));
    assert!(session.world.state().mobs[&golem].anims.contains(EMERGE));
    let feet = session.world.mob_pos(golem).unwrap();
    assert!((feet[1] + BURROW_DEPTH).abs() < 1e-9, "still under the ground");

    at_work(&mut session, id, |_, projects, job| {
        job.crew.step = Step::Emerge { t: EMERGE_TICKS };
        emerge(projects, job, &body);
        assert_eq!(job.crew.step, Step::Plan);
    });
    assert_eq!(
        session.builder.projects.get(id).map(|p| p.phase()),
        Some(Phase::Working)
    );
    assert_eq!(session.world.count(ContainerAddress::Mob(golem), BLUEPRINT), 1);
    assert_eq!(session.world.count(ContainerAddress::Block(TABLE_AT), BLUEPRINT), 0);
    assert!(session.world.state().mobs[&golem].anims.is_empty());
}

#[test]
fn a_golem_called_off_hands_its_plans_back_before_it_sinks() {
    let (mut session, id, golem) = working(HOME);
    session.builder.projects.update(id, |p| {
        p.cancel();
        p.burrow();
    });
    session
        .world
        .put(ContainerAddress::Block(TABLE_AT), 0, None);
    let body = session.body(golem);
    at_work(&mut session, id, |_, projects, job| {
        job.crew.step = Step::Burrow { t: 0 };
        burrow(projects, job, &body);
        assert_eq!(job.crew.step, Step::Burrow { t: 1 });
    });
    let tabled = session.world.container(ContainerAddress::Block(TABLE_AT))[0].clone();
    assert_eq!(
        tabled.as_ref().and_then(|s| session.builder.projects.bound(s)),
        Some(id)
    );
    assert_eq!(session.world.count(ContainerAddress::Mob(golem), BLUEPRINT), 0);

    let body = session.body(golem);
    at_work(&mut session, id, |_, projects, job| {
        job.crew.step = Step::Burrow { t: BURROW_TICKS };
        burrow(projects, job, &body);
        assert_eq!(job.crew.mob, None);
    });
    assert!(session.world.mob_pos(golem).is_none(), "gone into the ground");
    assert_eq!(
        session.builder.projects.get(id).map(|p| p.phase()),
        Some(Phase::Cancelled)
    );
}

#[test]
fn plans_the_table_has_no_room_for_are_left_on_the_ground() {
    let (mut session, id, golem) = working(HOME);
    session.builder.projects.update(id, |p| {
        p.cancel();
        p.burrow();
    });
    let body = session.body(golem);
    at_work(&mut session, id, |_, projects, job| {
        job.crew.step = Step::Burrow { t: 0 };
        burrow(projects, job, &body);
    });
    assert!(session
        .world
        .deeds()
        .contains(&Deed::Dropped(BLUEPRINT.into(), 1)));
    assert_eq!(session.world.count(ContainerAddress::Mob(golem), BLUEPRINT), 0);
}

#[test]
fn a_stuck_golem_sinks_travels_under_the_ground_and_rises_at_home() {
    let (mut session, id, golem) = working([8, 0, 8]);
    at_work(&mut session, id, |ctx, _, _| {
        ctx.routes
            .insert(([0; 3], [1; 3], 0), (Route::Open, ctx.now));
    });
    let mut step = Step::Relocate { t: 0, leg: 0 };
    let mut calls = 0;
    while let Step::Relocate { t, leg } = step {
        let body = session.body(golem);
        step = at_work(&mut session, id, |ctx, projects, job| {
            relocate(ctx, projects, job, &body, t, leg)
        });
        calls += 1;
        assert!(calls < 400, "the way home under the ground ends");
    }
    assert_eq!(step, Step::Plan);
    let feet = session.world.mob_pos(golem).unwrap();
    assert_eq!((feet[0], feet[2]), (-0.5, -3.5), "over home");
    assert!(feet[1].abs() < 0.05, "risen to the surface");
    at_work(&mut session, id, |ctx, _, job| {
        assert!(ctx.routes.is_empty(), "every route is judged afresh");
        assert_eq!(job.crew.rescue.burrow_from, [8, 0, 8]);
    });
}
