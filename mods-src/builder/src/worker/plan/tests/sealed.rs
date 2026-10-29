use super::*;
use crate::host::fake::rows::{AIR, DIRT};
use crate::survey::Known;
use crate::worker::act::settle;

#[test]
fn sealed_clearing_work_is_reopened_through_a_thick_wall_until_replaced() {
    let inner_cell = [5, 0, 5];
    let opener_cell = [5, 0, 4];
    let outer_cell = [5, 0, 3];
    let (mut session, id) = Session::site(
        "Sealed work",
        [6, 1, 6],
        &[
            (inner_cell, "petramond:stone"),
            (opener_cell, "petramond:stone"),
            (outer_cell, "petramond:stone"),
        ],
    );
    for cell in [
        [4, 0, 5],
        [6, 0, 5],
        [5, 0, 6],
        [5, 1, 5],
        [4, 0, 4],
        [6, 0, 4],
        [5, 1, 4],
        opener_cell,
        outer_cell,
    ] {
        session.world.set(cell, STONE);
    }
    session.world.set(inner_cell, DIRT);
    session.job(id);
    let golem = session.golem(id, HOME, [5, 0, 2]);
    session
        .world
        .give(ContainerAddress::Entity(golem), "petramond:stone", 2);
    let body = session.body(golem);
    let inner = unit(&session, id, inner_cell);
    let opener = unit(&session, id, opener_cell);
    let outer = unit(&session, id, outer_cell);
    let project = session.builder.projects.get(id).unwrap().clone();

    at_work(&mut session, id, |ctx, projects, job| {
        assert!(matches!(
            job.survey.as_ref().unwrap().known[inner],
            Known::Clear { .. }
        ));
        unreached::fall_out(
            ctx,
            projects,
            job,
            &body,
            &project,
            Task::Unit(inner),
            true,
            &[inner_cell],
        );
        let found = gather::gather(ctx, job, &body, &project, Mode::Building, None);
        assert!(found.list.iter().any(|(t, _)| *t == Task::Reopen(outer)));
    });

    session.world.set(outer_cell, AIR);
    at_work(&mut session, id, |ctx, projects, job| {
        settle(ctx, projects, job, Task::Reopen(outer));
        unreached::fall_out(
            ctx,
            projects,
            job,
            &body,
            &project,
            Task::Unit(inner),
            true,
            &[inner_cell],
        );
        let found = gather::gather(ctx, job, &body, &project, Mode::Building, None);
        assert!(found.list.iter().any(|(t, _)| *t == Task::Reopen(opener)));
    });

    session.world.set(opener_cell, AIR);
    at_work(&mut session, id, |ctx, projects, job| {
        settle(ctx, projects, job, Task::Reopen(opener));
        let found = gather::gather(ctx, job, &body, &project, Mode::Building, None);
        assert!(found.list.iter().any(|(t, _)| *t == Task::Unit(inner)));
        for o in [outer, opener] {
            assert!(
                viability(ctx, job, &body, Task::Unit(o), body.pos) == Viable::Waits(SEALED_WAIT),
                "the opening stays open while the obstruction needs clearing"
            );
        }
    });

    session.world.set(inner_cell, AIR);
    session.world.unload(inner_cell, inner_cell);
    at_work(&mut session, id, |ctx, _, job| {
        job.survey.as_mut().unwrap().measure(&job.design, &[inner]);
        gather::gather(ctx, job, &body, &project, Mode::Building, None);
        for o in [outer, opener] {
            assert!(
                viability(ctx, job, &body, Task::Unit(o), body.pos) == Viable::Waits(SEALED_WAIT),
                "unloaded work still needs its opening"
            );
        }
    });
    session.world.state_mut().unloaded.clear();
    at_work(&mut session, id, |ctx, projects, job| {
        settle(ctx, projects, job, Task::Unit(inner));
        let found = gather::gather(ctx, job, &body, &project, Mode::Building, None);
        assert!(found.list.iter().any(|(t, _)| *t == Task::Unit(inner)));
        for o in [outer, opener] {
            assert!(
                viability(ctx, job, &body, Task::Unit(o), body.pos) == Viable::Waits(SEALED_WAIT),
                "the opening stays open until the replacement is placed"
            );
        }
    });

    session.world.set(inner_cell, STONE);
    at_work(&mut session, id, |ctx, projects, job| {
        settle(ctx, projects, job, Task::Unit(inner));
        let found = gather::gather(ctx, job, &body, &project, Mode::Building, None);
        assert!(found.list.iter().any(|(t, _)| *t == Task::Unit(opener)));
        assert!(viability(ctx, job, &body, Task::Unit(opener), body.pos) == Viable::Now);
    });
}
