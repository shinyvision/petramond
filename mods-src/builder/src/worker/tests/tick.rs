use super::{at_work, unit};
use crate::host::fake::rows::STONE;
use crate::host::prelude::*;
use crate::testing::{Session, HOME};
use crate::worker::{tick, Step, Task};

const AT: [i32; 3] = [1, 0, -2];

#[test]
fn a_climb_refreshes_access_after_placing_its_previous_scaffold() {
    let (mut session, id) = Session::row();
    session.job(id);
    let golem = session.golem(id, HOME, [5, 2, 5]);
    session
        .world
        .give(ContainerAddress::Entity(golem), "petramond:dirt", 8);
    let mut body = session.body(golem);
    body.pos[1] = 2.1;
    body.on_ground = false;
    session.world.state_mut().mobs.get_mut(&golem).unwrap().pos = body.pos;
    let project = session.builder.projects.get(id).unwrap().clone();
    let world = std::rc::Rc::clone(&session.world);
    at_work(&mut session, id, |ctx, projects, job| {
        let pillar = crate::worker::Pillar {
            column: [5, 5],
            base: 0,
            top: 3,
            exit: [6, 0, 5],
            onward: None,
        };
        job.crew.step = Step::Climb {
            pillar,
            level: Some(1),
            placed: true,
            since: ctx.now,
        };
        world.set([5, 0, 5], STONE);
        job.access_plan.changed(&[[5, 0, 5]], false);
        assert_eq!(job.access_plan.score(), None);
        ctx.regions.clear();
        let nodes = *ctx.probe_nodes;
        *ctx.probe_nodes = 0;
        crate::worker::drive::act_out(ctx, projects, job, &project, &body);
        assert!(matches!(job.crew.step, Step::Climb { .. }));
        *ctx.probe_nodes = nodes;
        for _ in 0..30 {
            crate::worker::drive::act_out(ctx, projects, job, &project, &body);
            assert!(matches!(job.crew.step, Step::Climb { .. }));
            if projects.get(id).unwrap().scaffolds.contains(&[5, 1, 5]) {
                return;
            }
            ctx.now += 1;
        }
        panic!("the next scaffold must be placed after refreshing access");
    });
    assert_ne!(session.world.block([5, 1, 5]), crate::host::fake::rows::AIR);
}

fn middle_left() -> (Session, crate::project::ProjectId, u64) {
    let (mut session, id) = Session::row();
    session.world.set([0, 0, 0], STONE);
    session.world.set([2, 0, 0], STONE);
    session.job(id);
    let golem = session.golem(id, HOME, AT);
    session
        .world
        .give(ContainerAddress::Entity(golem), "petramond:stone", 3);
    (session, id, golem)
}

#[test]
fn a_golem_turns_to_a_block_lays_it_and_hears_it_land_over_a_few_ticks() {
    let (mut session, id, golem) = middle_left();
    let middle = unit(&session, id, [1, 0, 0]);
    at_work(&mut session, id, |ctx, projects, job| {
        tick(ctx, projects, job)
    });
    let until = match session.builder.jobs.map[&id].crew.step {
        Step::Aim { task, until } => {
            assert_eq!(task, Task::Unit(middle));
            until
        }
        other => panic!("turned to the block, not {other:?}"),
    };
    assert!(
        until > session.now(),
        "a hand never moves the instant it could"
    );
    assert_eq!(
        session.world.state().mobs[&golem].held.0.as_deref(),
        Some("petramond:stone"),
        "the block in hand"
    );

    session.world.set_now(until);
    at_work(&mut session, id, |ctx, projects, job| {
        tick(ctx, projects, job)
    });
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
