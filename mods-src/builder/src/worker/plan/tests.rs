//! Planning against the fake world: the stages one at a time where they
//! decide something alone, and whole plans for the golem's usual moments.

use super::sealing::{as_walls, swings_open};
use super::verdict::{settle_verdict, viability, Viable};
use super::*;
use crate::geometry::{feet_of, reaches};
use crate::host::fake::rows::STONE;
use crate::host::prelude::*;
use crate::project::{Hold, ProjectId};
use crate::testing::{Session, CHEST_AT, HOME};
use crate::worker::tests::{at_work, unit, working};
use crate::worker::tuning::waits::{FACELESS_SPACING, SEALED_WAIT};

/// In front of the middle of the row, in reach of all of it.
const AT: [i32; 3] = [1, 0, -2];

/// One plan for the golem as it stands now.
fn plan_now(session: &mut Session, id: ProjectId, golem: u64) -> Step {
    let body = session.body(golem);
    at_work(session, id, |ctx, projects, job| plan(ctx, projects, job, &body))
}

fn phase(session: &mut Session, id: ProjectId) -> Phase {
    session.builder.projects.get(id).unwrap().phase()
}

/// The row with its two ends built, and a golem in front of it.
fn middle_left(at: [i32; 3]) -> (Session, ProjectId, u64) {
    let (mut session, id) = Session::row();
    session.world.set([0, 0, 0], STONE);
    session.world.set([2, 0, 0], STONE);
    session.job(id);
    let golem = session.golem(id, HOME, at);
    (session, id, golem)
}

#[test]
fn the_mode_follows_the_phase_and_the_scaffolding_left() {
    let (mut session, id, _) = working(HOME);
    let mut project = session.builder.projects.get(id).unwrap().clone();
    assert_eq!(Mode::of(&project), Mode::Building);
    project.wind_down(String::new());
    assert_eq!(Mode::of(&project), Mode::GoingHome);
    project.scaffolds.push([4, 0, 4]);
    assert_eq!(Mode::of(&project), Mode::StrikingScaffold);
}

#[test]
fn a_tasks_cells_are_the_cells_it_works_on() {
    let (session, id, _) = working(HOME);
    let middle = unit(&session, id, [1, 0, 0]);
    let job = &session.builder.jobs.map[&id];
    assert_eq!(task_cells(job, Task::Unit(middle)), vec![[1, 0, 0]]);
    assert_eq!(task_cells(job, Task::Reopen(middle)), vec![[1, 0, 0]]);
    assert_eq!(task_cells(job, Task::Scaffold([4, 0, 4])), vec![[4, 0, 4]]);
    assert_eq!(
        task_cells(
            job,
            Task::Support {
                unit: middle,
                cell: [1, -1, 0]
            }
        ),
        vec![[1, -1, 0]]
    );
    assert!(places(job, Task::Unit(middle)));
    assert!(!places(job, Task::Scaffold([4, 0, 4])));
}

#[test]
fn a_body_in_the_air_or_a_site_not_yet_surveyed_plans_nothing() {
    let (mut session, id, golem) = working(AT);
    session
        .world
        .state_mut()
        .mobs
        .get_mut(&golem)
        .unwrap()
        .on_ground = false;
    assert_eq!(plan_now(&mut session, id, golem), Step::Plan);
    session
        .world
        .state_mut()
        .mobs
        .get_mut(&golem)
        .unwrap()
        .on_ground = true;
    session.builder.jobs.map.get_mut(&id).unwrap().survey = None;
    assert_eq!(plan_now(&mut session, id, golem), Step::Plan);
    assert_eq!(phase(&mut session, id), Phase::Working, "nothing was finished");
}

#[test]
fn work_in_sight_is_done_from_where_the_golem_stands() {
    let (mut session, id, golem) = middle_left(AT);
    session
        .world
        .give(ContainerAddress::Mob(golem), "petramond:stone", 3);
    let middle = unit(&session, id, [1, 0, 0]);
    let now = session.now();
    assert_eq!(
        plan_now(&mut session, id, golem),
        Step::Centre {
            task: Task::Unit(middle),
            since: now
        }
    );
}

/// The nearer block would stand between the golem and the one behind it.
#[test]
fn the_block_behind_goes_in_before_the_one_in_front_of_it() {
    let (mut session, id, golem) = working(AT);
    session
        .world
        .give(ContainerAddress::Mob(golem), "petramond:stone", 3);
    let west = unit(&session, id, [0, 0, 0]);
    let now = session.now();
    assert_eq!(
        plan_now(&mut session, id, golem),
        Step::Centre {
            task: Task::Unit(west),
            since: now
        }
    );
}

#[test]
fn work_out_of_reach_is_walked_to_a_stance_that_reaches_it() {
    let (mut session, id, golem) = working([-8, 0, -8]);
    session
        .world
        .give(ContainerAddress::Mob(golem), "petramond:stone", 3);
    let step = plan_now(&mut session, id, golem);
    let Step::Walk {
        goal,
        then: Then::Task(Task::Unit(i)),
        ..
    } = step
    else {
        panic!("a walk to work, not {step:?}");
    };
    let pos = session.builder.jobs.map[&id].design.units[i].pos;
    assert!(reaches(feet_of(goal), &[pos]));
    assert!(session.world.foothold(goal));
}

#[test]
fn with_nothing_in_hand_the_golem_fetches_from_the_chest() {
    let (mut session, id, golem) = working(AT);
    session
        .world
        .give(ContainerAddress::Block(CHEST_AT), "petramond:stone", 3);
    let now = session.now();
    assert_eq!(
        plan_now(&mut session, id, golem),
        Step::Walk {
            to: AT,
            goal: AT,
            then: Then::Fetch(CHEST_AT),
            best: 0,
            progress: now
        },
        "the chest is in reach from here"
    );
    assert_eq!(session.builder.jobs.map[&id].crew.why, Waiting::Resupply);
}

#[test]
fn with_nothing_anywhere_the_job_waits_for_supplies() {
    let (mut session, id, golem) = working(AT);
    assert_eq!(plan_now(&mut session, id, golem), Step::Plan);
    let project = session.builder.projects.get(id).unwrap();
    assert_eq!(project.hold(), Some(Hold::Supplies));
    assert_eq!(project.note, "Missing 3x Stone");
}

#[test]
fn work_set_aside_is_waited_for() {
    let (mut session, id, golem) = working(AT);
    session
        .world
        .give(ContainerAddress::Mob(golem), "petramond:stone", 3);
    let job = session.builder.jobs.map.get_mut(&id).unwrap();
    for i in 0..3 {
        job.crew.deferrals.defer(Task::Unit(i), 1000);
    }
    assert_eq!(plan_now(&mut session, id, golem), Step::Plan);
    let job = &session.builder.jobs.map[&id];
    assert_eq!(job.crew.why, Waiting::DeferredOrUnloaded);
    assert_eq!(phase(&mut session, id), Phase::Working);
}

#[test]
fn with_everything_built_the_golem_winds_down_and_heads_home() {
    let (mut session, id) = Session::row();
    session.world.fill([0, 0, 0], [2, 0, 0], STONE);
    session.job(id);
    let golem = session.golem(id, HOME, [3, 0, -5]);
    assert_eq!(plan_now(&mut session, id, golem), Step::Plan);
    assert_eq!(phase(&mut session, id), Phase::Returning);
    assert_eq!(session.builder.projects.get(id).unwrap().note, "");
    let now = session.now();
    assert_eq!(
        plan_now(&mut session, id, golem),
        Step::Walk {
            to: HOME,
            goal: HOME,
            then: Then::Home,
            best: i32::MAX,
            progress: now
        }
    );

    // What it carries goes back in the chests before it goes.
    session
        .world
        .give(ContainerAddress::Mob(golem), "petramond:dirt", 5);
    assert_eq!(
        plan_now(&mut session, id, golem),
        Step::Walk {
            to: [3, 0, -5],
            goal: [3, 0, -5],
            then: Then::Deposit(CHEST_AT),
            best: 0,
            progress: now
        }
    );
}

#[test]
fn at_home_with_the_work_done_the_golem_burrows() {
    let (mut session, id) = Session::row();
    session.world.fill([0, 0, 0], [2, 0, 0], STONE);
    session.job(id);
    let golem = session.golem(id, HOME, HOME);
    session
        .builder
        .projects
        .update(id, |p| p.wind_down(String::new()));
    assert_eq!(plan_now(&mut session, id, golem), Step::Burrow { t: 0 });
    assert_eq!(phase(&mut session, id), Phase::Burrowing);
}

#[test]
fn the_report_says_what_was_lost_and_what_was_left_standing() {
    let (mut session, id) = Session::row();
    session.world.fill([0, 0, 0], [2, 0, 0], STONE);
    session.job(id);
    session.golem(id, HOME, HOME);
    session.world.set([0, 0, 0], crate::host::fake::rows::AIR);
    let west = unit(&session, id, [0, 0, 0]);
    let middle = unit(&session, id, [1, 0, 0]);
    let Session { builder, .. } = &mut session;
    let job = builder.jobs.map.get_mut(&id).unwrap();
    job.crew.built.extend([west, middle]);
    job.crew.scaffolding.left_standing = 2;
    job.crew.note = "stale".into();
    let survey = job.survey.as_mut().unwrap();
    survey.measure(&job.design, &[west]);
    assert_eq!(finish(&mut builder.projects, job), Step::Plan);
    assert!(job.crew.note.is_empty());
    let project = builder.projects.get(id).unwrap();
    assert_eq!(project.phase(), Phase::Returning);
    assert_eq!(
        project.note,
        "1 block was lost after it was placed; 2 scaffolds were out of reach and stand"
    );
}

#[test]
fn the_world_is_asked_whether_a_block_would_go_in_before_anyone_is_sent() {
    let (mut session, id, golem) = working(AT);
    let middle = Task::Unit(unit(&session, id, [1, 0, 0]));
    let body = session.body(golem);
    let far = feet_of([-8, 0, -8]);
    at_work(&mut session, id, |ctx, _, job| {
        assert!(
            viability(ctx, job, &body, middle, body.pos) == Viable::Elsewhere,
            "nothing in hand: fetched, then laid"
        );
    });
    session
        .world
        .give(ContainerAddress::Mob(golem), "petramond:stone", 3);
    let body = session.body(golem);
    at_work(&mut session, id, |ctx, _, job| {
        assert!(viability(ctx, job, &body, middle, body.pos) == Viable::Now);
        assert!(viability(ctx, job, &body, middle, far) == Viable::Elsewhere);
        assert!(viability(ctx, job, &body, Task::Scaffold([4, 0, 4]), far) == Viable::Now);
    });
    session.world.set([1, 0, 0], STONE);
    at_work(&mut session, id, |ctx, _, job| {
        assert!(viability(ctx, job, &body, middle, body.pos) == Viable::Done);
        settle_verdict(ctx, job, middle, Viable::Done);
        let Task::Unit(i) = middle else { unreachable!() };
        assert_eq!(job.survey.as_ref().unwrap().known[i], crate::survey::Known::Satisfied);
        settle_verdict(ctx, job, Task::Scaffold([4, 0, 4]), Viable::Waits(50));
        assert!(job.crew.deferrals.deferred(Task::Scaffold([4, 0, 4]), ctx.now + 49));
    });
}

#[test]
fn a_block_with_nothing_beside_it_waits_then_gets_a_prop() {
    let (mut session, id) = Session::site("Float", [1, 3, 1], &[([0, 2, 0], "petramond:stone")]);
    session.job(id);
    let golem = session.golem(id, HOME, [1, 0, 0]);
    session
        .world
        .give(ContainerAddress::Mob(golem), "petramond:stone", 1);
    let floating = unit(&session, id, [0, 2, 0]);
    let body = session.body(golem);
    at_work(&mut session, id, |ctx, _, job| {
        let task = Task::Unit(floating);
        let start = ctx.now;
        assert!(viability(ctx, job, &body, task, body.pos) == Viable::Waits(SEALED_WAIT));
        ctx.now = start + FACELESS_SPACING;
        assert!(viability(ctx, job, &body, task, body.pos) == Viable::Waits(SEALED_WAIT));
        assert!(!job.crew.faces.floating.contains(&floating));
        ctx.now = start + 2 * FACELESS_SPACING;
        assert!(viability(ctx, job, &body, task, body.pos) == Viable::Waits(0));
        assert!(job.crew.faces.floating.contains(&floating));
    });
}

#[test]
fn a_thin_or_tall_block_is_walled_as_high_as_a_body_cannot_step() {
    let (mut session, id) = Session::site(
        "Fencing",
        [4, 2, 1],
        &[
            ([0, 0, 0], "petramond:oak_fence"),
            ([1, 0, 0], "petramond:glass_pane"),
            ([2, 0, 0], "petramond:stone"),
            ([3, 0, 0], "petramond:oak_door"),
        ],
    );
    session.job(id);
    session.golem(id, HOME, HOME);
    let door = unit(&session, id, [3, 0, 0]);
    let stone = unit(&session, id, [2, 0, 0]);
    at_work(&mut session, id, |ctx, _, job| {
        assert_eq!(as_walls(ctx, job, &[[0, 0, 0]]), vec![[0, 0, 0], [0, 1, 0]]);
        assert_eq!(as_walls(ctx, job, &[[1, 0, 0]]), vec![[1, 0, 0], [1, 1, 0]]);
        assert_eq!(as_walls(ctx, job, &[[2, 0, 0]]), vec![[2, 0, 0]]);
        assert_eq!(
            as_walls(ctx, job, &[[3, 0, 0], [3, 1, 0]]),
            vec![[3, 0, 0], [3, 1, 0]]
        );
        assert!(swings_open(ctx.caches, &job.design, door));
        assert!(!swings_open(ctx.caches, &job.design, stone));
    });
}
