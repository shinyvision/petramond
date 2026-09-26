//! Whole ticks of the worker, and the upkeep every tick does.

use super::{at_work, unit};
use crate::host::fake::rows::{AIR, GRASS, STONE, TORCH, WATER};
use crate::host::prelude::*;
use crate::project::{Hold, Note, Phase};
use crate::testing::{Session, HOME};
use crate::worker::upkeep::{hold_for_blueprint, mend, open_block};
use crate::worker::{tick, Step, Task, FULL_HEALTH_TAG, HEALTH_TAG};

const AT: [i32; 3] = [1, 0, -2];

/// The row with only its middle left to lay, and a golem in front of it
/// carrying the stone.
fn middle_left() -> (Session, crate::project::ProjectId, u64) {
    let (mut session, id) = Session::row();
    session.world.set([0, 0, 0], STONE);
    session.world.set([2, 0, 0], STONE);
    session.job(id);
    let golem = session.golem(id, HOME, AT);
    session
        .world
        .give(ContainerAddress::Mob(golem), "petramond:stone", 3);
    (session, id, golem)
}

#[test]
fn a_golem_turns_to_a_block_lays_it_and_hears_it_land_over_a_few_ticks() {
    let (mut session, id, golem) = middle_left();
    let middle = unit(&session, id, [1, 0, 0]);
    at_work(&mut session, id, |ctx, projects, job| tick(ctx, projects, job));
    let until = match session.builder.jobs.map[&id].crew.step {
        Step::Aim { task, until } => {
            assert_eq!(task, Task::Unit(middle));
            until
        }
        other => panic!("turned to the block, not {other:?}"),
    };
    assert!(until > session.now(), "a hand never moves the instant it could");
    assert_eq!(
        session.world.state().mobs[&golem].held.0.as_deref(),
        Some("petramond:stone"),
        "the block in hand"
    );

    session.world.set_now(until);
    at_work(&mut session, id, |ctx, projects, job| tick(ctx, projects, job));
    assert_eq!(
        session.builder.jobs.map[&id].crew.step,
        Step::Await {
            task: Task::Unit(middle),
            since: until
        }
    );
    assert_eq!(session.world.block([1, 0, 0]), STONE);

    session
        .builder
        .acted(golem, [1, 0, 0], ActorAction::Place, None);
    let job = &session.builder.jobs.map[&id];
    assert_eq!(job.crew.step, Step::Plan);
    assert!(job.crew.built.contains(&middle));
    assert_eq!(job.done(), 1.0);
}

#[test]
fn a_tick_passes_over_a_job_with_no_golem_out() {
    let (mut session, id, golem) = middle_left();
    session.builder.projects.update(id, |p| p.golem_died());
    at_work(&mut session, id, |ctx, projects, job| tick(ctx, projects, job));
    assert_eq!(session.builder.jobs.map[&id].crew.step, Step::Plan);
    assert!(session.world.state().mobs[&golem].held.0.is_none(), "nothing done");
}

#[test]
fn a_golem_without_its_blueprint_holds_its_job_until_it_is_back() {
    let (mut session, id, golem) = middle_left();
    let blueprint = session.world.container(ContainerAddress::Mob(golem))[0].clone();
    session.world.put(ContainerAddress::Mob(golem), 0, None);
    let body = session.body(golem);
    let project = session.builder.projects.get(id).unwrap().clone();
    hold_for_blueprint(&mut session.builder.projects, &project, &body);
    let held = session.builder.projects.get(id).unwrap().clone();
    assert_eq!((held.hold(), &held.note), (Some(Hold::Blueprint), &Note::MissingBlueprint));

    session.world.put(ContainerAddress::Mob(golem), 0, blueprint);
    let body = session.body(golem);
    hold_for_blueprint(&mut session.builder.projects, &held, &body);
    let project = session.builder.projects.get(id).unwrap();
    assert_eq!((project.hold(), &project.note), (None, &Note::None));
    assert_eq!(project.phase(), Phase::Working);
}

#[test]
fn a_golem_mends_toward_the_health_it_was_summoned_with() {
    let (session, _, golem) = middle_left();
    mend(golem, 50.0);
    assert_eq!(session.world.tag(golem, HEALTH_TAG), None, "no full health known");
    session
        .world
        .state_mut()
        .mobs
        .get_mut(&golem)
        .unwrap()
        .tags
        .insert(FULL_HEALTH_TAG.into(), MobTagValue::F64(60.0));
    mend(golem, 50.0);
    assert_eq!(session.world.tag(golem, HEALTH_TAG), Some(MobTagValue::F64(51.0)));
    mend(golem, 59.5);
    assert_eq!(session.world.tag(golem, HEALTH_TAG), Some(MobTagValue::F64(60.0)));
    session.world.state_mut().mobs.get_mut(&golem).unwrap().tags.remove(HEALTH_TAG);
    mend(golem, 60.0);
    assert_eq!(session.world.tag(golem, HEALTH_TAG), None, "whole already");
}

#[test]
fn open_room_is_what_a_body_passes_through_and_no_fluid() {
    let (mut session, id, _) = middle_left();
    at_work(&mut session, id, |ctx, _, _| {
        assert!(open_block(ctx, AIR));
        assert!(open_block(ctx, GRASS));
        assert!(open_block(ctx, TORCH));
        assert!(!open_block(ctx, STONE));
        assert!(!open_block(ctx, WATER));
    });
}
