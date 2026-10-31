use super::{at_work, unit, working};
use crate::host::fake::rows::{AIR, DIRT, STONE};
use crate::host::prelude::*;
use crate::project::Projects;
use crate::survey::Known;
use crate::worker::act::{dig, settle};
use crate::worker::tuning::waits::FACELESS_SPACING;
use crate::worker::Job;
use crate::worker::{acted, Ctx, Step, Task};

const AT: [i32; 3] = [1, 0, -2];

fn refuse(
    ctx: &mut Ctx,
    projects: &mut Projects,
    job: &mut Job,
    task: Task,
    pos: [i32; 3],
    refusal: ActionRefusal,
) {
    job.crew.step = Step::Await {
        task,
        since: ctx.now,
    };
    acted(ctx, projects, job, pos, ActorAction::Place, Some(refusal));
    assert_eq!(job.crew.step, Step::Plan);
}

#[test]
fn a_placement_landing_settles_its_unit_and_wakes_its_neighbours() {
    let (mut session, id, _) = working(AT);
    let west = unit(&session, id, [0, 0, 0]);
    let middle = unit(&session, id, [1, 0, 0]);
    let east = unit(&session, id, [2, 0, 0]);
    session.world.set([1, 0, 0], STONE);
    at_work(&mut session, id, |ctx, projects, job| {
        job.crew.step = Step::Await {
            task: Task::Unit(middle),
            since: 1,
        };
        job.crew.deferrals.defer(Task::Unit(west), 500);
        job.crew.deferrals.defer(Task::Unit(east), 500);
        ctx.routes
            .insert(([0; 3], [5; 3], 0), (Route::Open, ctx.now));
        ctx.routes
            .insert(([0; 3], [6; 3], 0), (Route::Closed, ctx.now));
        acted(ctx, projects, job, [9, 9, 9], ActorAction::Place, None);
        assert!(
            matches!(job.crew.step, Step::Await { .. }),
            "another cell's outcome"
        );
        assert_eq!(
            ctx.routes.len(),
            1,
            "a block laid anywhere may close a route"
        );
        acted(ctx, projects, job, [1, 0, 0], ActorAction::Place, None);
        assert_eq!(job.crew.step, Step::Plan);
        assert!(job.crew.built.contains(&middle));
        assert_eq!(job.survey.as_ref().unwrap().known[middle], Known::Satisfied);
        assert!(!job.crew.deferrals.deferred(Task::Unit(west), ctx.now));
        assert!(!job.crew.deferrals.deferred(Task::Unit(east), ctx.now));
        assert_eq!(job.crew.pace.progress_at, ctx.now);
    });
}

#[test]
fn a_block_with_nothing_to_hang_on_is_propped_once_refused_often_enough() {
    let (mut session, id, _) = working(AT);
    let middle = unit(&session, id, [1, 0, 0]);
    let task = Task::Unit(middle);
    at_work(&mut session, id, |ctx, projects, job| {
        let tries = [1, 1 + FACELESS_SPACING, 1 + 2 * FACELESS_SPACING];
        for (k, now) in tries.into_iter().enumerate() {
            ctx.now = now;
            refuse(ctx, projects, job, task, [1, 0, 0], ActionRefusal::NoFace);
            assert_eq!(job.crew.faces.floating.contains(&middle), k == 2);
        }
        assert!(job.crew.deferrals.deferred(task, tries[1] + 119));
        assert!(!job.crew.deferrals.deferred(task, tries[1] + 120));
    });
}

#[test]
fn a_dug_block_leaves_the_scaffold_records_and_forgets_shut_routes_near_it() {
    let (mut session, id, _) = working(AT);
    session
        .builder
        .projects
        .update(id, |p| p.scaffolds = vec![[4, 0, 4], [5, 0, 5]]);
    at_work(&mut session, id, |ctx, projects, job| {
        ctx.routes
            .insert(([4, 0, 3], [9, 0, 9], 0), (Route::Closed, ctx.now));
        ctx.routes
            .insert(([-9, 0, -9], [-8, 0, -9], 0), (Route::Closed, ctx.now));
        ctx.routes
            .insert(([4, 0, 3], [4, 0, 2], 0), (Route::Open, ctx.now));
        acted(ctx, projects, job, [4, 0, 4], ActorAction::Dig, None);
        assert_eq!(projects.peek(id).unwrap().scaffolds, vec![[5, 0, 5]]);
        assert_eq!(ctx.routes.len(), 2, "only the shut answer near the dig");
    });
}

#[test]
fn a_dig_in_reach_breaks_the_block_and_waits_on_the_outcome() {
    let (mut session, id, golem) = working(AT);
    session.world.set([1, 0, -1], DIRT);
    let body = session.body(golem);
    at_work(&mut session, id, |ctx, projects, job| {
        let cut = Task::Trim([1, 0, -1]);
        let step = dig(ctx, projects, job, &body, cut, [1, 0, -1], ctx.now);
        assert_eq!(
            step,
            Step::Await {
                task: cut,
                since: ctx.now
            }
        );
        let air = Task::Trim([3, 1, -1]);
        let step = dig(ctx, projects, job, &body, air, [3, 1, -1], ctx.now);
        assert_eq!(step, Step::Plan, "nothing there to dig");
        assert!(job.crew.deferrals.deferred(air, ctx.now));
    });
    assert_eq!(session.world.block([1, 0, -1]), AIR);
    assert_eq!(
        session
            .world
            .count(ContainerAddress::Entity(golem), "petramond:dirt"),
        0,
        "cut overgrowth is not carried off"
    );
}

#[test]
fn settling_scaffolding_overgrowth_and_breakouts_keeps_the_records_true() {
    let (mut session, id, _) = working(AT);
    let east = unit(&session, id, [2, 0, 0]);
    session
        .builder
        .projects
        .update(id, |p| p.scaffolds = vec![[4, 0, 4], [5, 0, 5]]);
    session.world.set([5, 0, 5], DIRT);
    at_work(&mut session, id, |ctx, projects, job| {
        settle(ctx, projects, job, Task::Scaffold([4, 0, 4]));
        settle(ctx, projects, job, Task::Scaffold([5, 0, 5]));
        assert_eq!(
            projects.peek(id).unwrap().scaffolds,
            vec![[5, 0, 5]],
            "a scaffold still standing stays on the record"
        );

        job.crew.access.trims.insert([6, 1, 6]);
        settle(ctx, projects, job, Task::Trim([6, 1, 6]));
        assert!(job.crew.access.trims.is_empty());

        job.crew.access.digs.insert([2, 0, 0]);
        job.crew.built.insert(east);
        settle(ctx, projects, job, Task::Breakout([2, 0, 0]));
        assert!(job.crew.access.digs.is_empty());
        assert!(!job.crew.built.contains(&east), "it is laid again");
    });
}
