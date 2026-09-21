//! Work the golem can do from where it stands.

use super::gather::leans_on_scaffold;
use super::sealing::{cutting, sealed_by};
use super::verdict::{settle_verdict, viability, Viable};
use super::{defer_task, places, task_cells, Flow, Mode, Round};
use crate::geometry::{beside, feet_of, manhattan, offset, reaches};
use crate::jobs::Job;
use crate::project::Projects;
use crate::survey::Known;
use crate::worker::tuning::waits::SEALED_WAIT;
use crate::worker::tuning::window::{HERE, LATE_CLEAR};
use crate::worker::waiting::{Probe, Waiting};
use crate::worker::{cargo, sight, Body, Ctx, Step, Task};

/// A block still to be laid right behind `task`'s, as the golem looks at it,
/// that could go in from here now: it goes first. The nearer block would
/// stand between the golem and it — and a block that must be turned the way
/// the golem looks (a stair under a gable) is laid through the very gap the
/// nearer one fills, or never: walled in, it cost a wall taken back down.
pub(super) fn behind(
    ctx: &mut Ctx,
    job: &mut Job,
    project: &crate::project::Project,
    mode: Mode,
    body: &Body,
    task: Task,
) -> Option<Task> {
    // A job going home lays nothing more: it only takes its scaffolding down.
    if mode != Mode::Building {
        return None;
    }
    let Task::Unit(i) = task else {
        return None;
    };
    if !places(job, task) {
        return None;
    }
    let carried = cargo::totals(&body.slots);
    let away = |c: [i32; 3]| {
        let at = feet_of(c);
        (0..3).map(|a| (at[a] - body.pos[a]).powi(2)).sum::<f64>()
    };
    let cells = job.design.cells(job.design.units[i]);
    let nearest = cells.iter().map(|c| away(*c)).fold(f64::MAX, f64::min);
    let mut beyond: Vec<usize> = beside(&cells)
        .filter(|n| away(*n) > nearest)
        .filter_map(|n| job.design.unit_at(n))
        .filter(|j| *j != i)
        .collect();
    beyond.sort_unstable();
    beyond.dedup();
    for j in beyond {
        let other = Task::Unit(j);
        let due = matches!(
            job.survey.as_ref().map(|s| &s.known[j]),
            Some(Known::Place(missing)) if cargo::holds(&carried, missing)
        );
        // Work whose turn has not come keeps waiting for it.
        let unit = job.design.units[j];
        let held_back = job.crew.pace.band_top.is_some_and(|top| unit.pos[1] > top)
            || job.design.passage(j)
            || job.crew.faces.floating.contains(&j)
            || (job.design.glazing(j)
                && !job.crew.glazing.under_way
                && job.crew.glazing.ways.value.contains(&j))
            || leans_on_scaffold(&job.design, unit, &project.scaffolds);
        if !due
            || held_back
            || job.crew.built.contains(&j)
            || job.crew.deferrals.deferred(other, ctx.now)
            || job.crew.deferrals.blind(other, body.cell)
            || job.crew.deferrals.tried.contains(&j)
        {
            continue;
        }
        let cells = task_cells(job, other);
        if !sees_from_here(job, other, body, &cells)
            || viability(ctx, job, body, other, body.pos) != Viable::Now
        {
            continue;
        }
        let safe = if job.crew.aloft.perch.is_some() {
            cutting(ctx, job, project, other, &cells)
        } else {
            sealed_by(ctx, job, project, body.cell, other, &cells)
        };
        if safe == Some(false) {
            return Some(other);
        }
    }
    None
}

/// Whether a body standing at `feet` keeps out of `cells` and off them.
pub(in crate::worker) fn clear_of_body(feet: [i32; 3], cells: &[[i32; 3]]) -> bool {
    !cells
        .iter()
        .any(|c| *c == feet || *c == offset(feet, [0, 1, 0]) || *c == offset(feet, [0, -1, 0]))
}

/// Whether the golem sees `cells` from where it stands.
pub(in crate::worker) fn sees_from_here(
    job: &Job,
    task: Task,
    body: &Body,
    cells: &[[i32; 3]],
) -> bool {
    reaches(body.pos, cells)
        && clear_of_body(body.cell, cells)
        && sight::sees_from(body, vec![body.pos], cells, &sight::work(job, task))[0].is_none()
}

/// Work seen from where the golem stands is done from there.
pub(super) fn from_here(
    ctx: &mut Ctx,
    _projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    round: &mut Round,
) -> Flow {
    let project = &round.project;
    let candidates = &round.candidates;
    let urgent = |task: &Task| task.urgent(&job.crew.scaffolding.urgent);
    let late = |task: &Task| task.late(&job.design);
    let mut here: Vec<(Task, [i32; 3])> = candidates
        .iter()
        .filter(|(task, c)| {
            !late(task)
                || !candidates
                    .iter()
                    .any(|(other, o)| !late(other) && manhattan(*o, *c) <= LATE_CLEAR)
        })
        .copied()
        .collect();
    here.sort_by_key(|(task, c)| (!urgent(task), manhattan(*c, body.cell)));
    for (task, _) in here.iter().take(HERE) {
        let cells = task_cells(job, *task);
        if job.crew.deferrals.blind(*task, body.cell) || !sees_from_here(job, *task, body, &cells) {
            continue;
        }
        let verdict = viability(ctx, job, body, *task, body.pos);
        if verdict != Viable::Now {
            settle_verdict(ctx, job, *task, verdict);
            continue;
        }
        match sealed_by(ctx, job, project, body.cell, *task, &cells) {
            Some(false) => {
                let task = behind(ctx, job, project, round.mode, body, *task).unwrap_or(*task);
                return Flow::Go(Step::Centre {
                    task,
                    since: ctx.now,
                });
            }
            Some(true) => defer_task(ctx, job, *task, SEALED_WAIT),
            None => {
                return Flow::Busy(Waiting::Probe(Probe::Sealing));
            }
        }
    }
    Flow::Pass
}
