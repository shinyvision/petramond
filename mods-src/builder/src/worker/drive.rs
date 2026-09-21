//! Running the step the golem is in, and deciding the next.

use mod_sdk::*;

use super::tuning::body::{COURSE_SETTLE_TICKS, STANCE_OFF_CENTRE};
use super::tuning::every::UNWEDGE_EVERY;
use super::tuning::patience::{AWAIT_TICKS, CENTRE_TICKS, IDLE_NOTE};
use super::{
    act, bridge, cargo, centre, eye_of, lifecycle, pillar, plan, rescue, unwedge, walk, wayin,
    Body, Ctx, Step, Then, TRACE,
};
use crate::jobs::Job;
use crate::project::{Phase, Project, Projects};

pub(super) fn act_out(
    ctx: &mut Ctx,
    projects: &mut Projects,
    job: &mut Job,
    project: &Project,
    body: &Body,
) {
    let id = body.id;
    match project.phase() {
        Phase::Emerging => lifecycle::emerge(projects, job, body),
        Phase::Burrowing => lifecycle::burrow(projects, job, body),
        Phase::Working | Phase::Returning => {
            // A golem someone is talking to stays where it is, as a held
            // one does.
            let asked = ctx
                .asked
                .iter()
                .find(|(mob, _)| *mob == id)
                .map(|(_, who)| *who);
            if (project.hold().is_some() || asked.is_some())
                && !matches!(job.crew.step, Step::Await { .. } | Step::Relocate { .. })
            {
                job.crew.presence.rest(id);
                if let Step::Climb { .. } | Step::Descend { .. } = job.crew.step {
                    centre(body);
                } else {
                    job.crew.step = Step::Plan;
                }
                if let Some(eye) = asked.and_then(eye_of) {
                    job.crew.presence.look_at(id, ctx.now, body.pos, eye);
                }
                return;
            }
            if matches!(job.crew.step, Step::Plan | Step::Walk { .. })
                && ctx.now.is_multiple_of(UNWEDGE_EVERY)
                && unwedge(ctx, body)
            {
                job.crew.trail.broken();
                job.crew.presence.set_goal(id, None);
                job.crew.step = Step::Plan;
                return;
            }
            // Deciding, turning to the work and starting on it are one moment,
            // not three ticks. (Placements still wait out their aim.)
            for _ in 0..3 {
                let before = job.crew.step;
                let step = run(ctx, projects, job, body, before);
                // Standing about with nothing to show for it: say what it waits on.
                if matches!(step, Step::Plan)
                    && matches!(before, Step::Plan)
                    && ctx.now > job.crew.pace.progress_at + IDLE_NOTE
                    && job.crew.note.is_empty()
                {
                    if let Some(waiting) = job.crew.why.told() {
                        job.crew.note = waiting.into();
                    }
                }
                let retarget = matches!(
                    (step, before),
                    (Step::Walk { to: a, then: x, .. }, Step::Walk { to: b, then: y, .. }) if a != b || x != y
                );
                if TRACE
                    && (retarget
                        || std::mem::discriminant(&step) != std::mem::discriminant(&before))
                {
                    log(&format!(
                        "TRACE t={} at {:?} {:?} -> {:?} note={}",
                        ctx.now, body.cell, before, step, job.crew.note
                    ));
                }
                job.crew.step = step;
                let at_once = matches!(
                    (before, step),
                    (Step::Plan, Step::Centre { .. })
                        | (Step::Centre { .. }, Step::Aim { .. } | Step::Dig { .. })
                );
                if !at_once {
                    break;
                }
            }
        }
        _ => {}
    }
}

fn run(ctx: &mut Ctx, projects: &mut Projects, job: &mut Job, body: &Body, step: Step) -> Step {
    match step {
        Step::Plan => {
            job.crew.presence.unaimed = 0;
            if let Some(door) = job.crew.access.pending_use.take() {
                return Step::Use {
                    door,
                    open: true,
                    since: ctx.now,
                };
            }
            if job.crew.aloft.perch.is_some() {
                centre(body);
            }
            plan::plan(ctx, projects, job, body)
        }
        Step::Use { door, open, since } => wayin::use_door(ctx, job, body, door, open, since),
        Step::Walk {
            to,
            goal,
            then,
            best,
            progress,
        } => walk(ctx, projects, job, body, to, goal, then, best, progress),
        Step::Centre { task, since } => {
            let beside = plan::task_cells(job, task).iter().any(|c| {
                (c[1] == body.cell[1] || c[1] == body.cell[1] + 1)
                    && (c[0] - body.cell[0]).abs() <= 1
                    && (c[2] - body.cell[2]).abs() <= 1
            });
            let centred = body
                .off_centre()
                .iter()
                .all(|d| d.abs() < STANCE_OFF_CENTRE);
            // Settle to the centre when laying right beside the body, or when
            // the work is not in sight: a stance is chosen for what its cell's
            // middle sees.
            let settle = !centred && {
                let cells = plan::task_cells(job, task);
                beside || !plan::sees_from_here(job, task, body, &cells)
            };
            if !settle || ctx.now > since + CENTRE_TICKS {
                match act::aim(ctx, job, body, task) {
                    Some(until) => Step::Aim { task, until },
                    None => act::begin(ctx, projects, job, body, task),
                }
            } else {
                centre(body);
                step
            }
        }
        Step::Rummage {
            container,
            deposit,
            since,
            moved,
        } => cargo::rummage(ctx, projects, job, body, container, deposit, since, moved),
        Step::Aim { task, until } => {
            if ctx.now >= until {
                act::begin(ctx, projects, job, body, task)
            } else {
                step
            }
        }
        Step::Dig { task, cell, since } => act::dig(ctx, projects, job, body, task, cell, since),
        Step::Await { task, since } => {
            if ctx.now > since + AWAIT_TICKS {
                act::settle(ctx, projects, job, task);
                return Step::Plan;
            }
            step
        }
        Step::Climb {
            pillar,
            level,
            placed,
            since,
        } => pillar::climb(ctx, projects, job, body, pillar, level, placed, since),
        Step::Descend { since } => pillar::descend(ctx, projects, job, body, since),
        Step::Hop { to, since } => rescue::hop(job, body, to, since, ctx.now),
        Step::Rise { from, since } => rescue::rise(ctx, projects, job, body, from, since),
        Step::Relocate { t, leg } => lifecycle::relocate(ctx, projects, job, body, t, leg),
        Step::Bridge { since, asked } => bridge::extend(ctx, projects, job, body, since, asked),
        Step::Unbridge { since } => bridge::retract(ctx, projects, job, body, since),
        Step::Emerge { .. } | Step::Burrow { .. } => Step::Plan,
    }
}

pub(super) fn arrive(
    ctx: &mut Ctx,
    projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    then: Then,
) -> Step {
    match then {
        Then::Task(task) => Step::Centre {
            task,
            since: ctx.now,
        },
        Then::Fetch(container) | Then::Deposit(container) => {
            cargo::open(ctx, job, body, container, matches!(then, Then::Deposit(_)))
        }
        Then::Climb(pillar) => Step::Climb {
            pillar,
            level: None,
            placed: false,
            since: ctx.now,
        },
        Then::Home => {
            projects.update(job.id, crate::project::Project::burrow);
            Step::Burrow { t: 0 }
        }
        Then::Regroup => Step::Plan,
        Then::Course(_) => {
            job.crew.aloft.settle_until = ctx.now + COURSE_SETTLE_TICKS;
            Step::Plan
        }
        Then::Shut(door) => Step::Use {
            door,
            open: false,
            since: ctx.now,
        },
        Then::Escape => Step::Plan,
        Then::Open(door) => Step::Use {
            door,
            open: true,
            since: ctx.now,
        },
    }
}
