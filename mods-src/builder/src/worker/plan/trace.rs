//! The plan's longer diagnostics, kept out of the way of what they describe.

use std::collections::BTreeMap;

use mod_sdk::*;

use super::here::sees_from_here;
use super::task_cells;
use super::verdict::{verdict_name, viability};
use crate::design::Design;
use crate::geometry::reaches;
use crate::jobs::Job;
use crate::survey::{ItemKey, Known, Survey};
use crate::worker::crew::Crew;
use crate::worker::step::Task;
use crate::worker::{cargo, Body, Ctx, TRACE};

pub(super) fn heartbeat(ctx: &Ctx, job: &Job, body: &Body) {
    if !TRACE || !ctx.now.is_multiple_of(400) {
        return;
    }
    log(&format!(
        "TRACE heartbeat t={} at {:?} waiting on {} stuck {} way_in {:?} shut out {} deferred {}",
        ctx.now,
        body.cell,
        job.crew.why.label(),
        job.crew.rescue.stuck.is_some(),
        job.crew
            .access
            .way_in
            .as_ref()
            .map(|(w, _)| w.first().copied()),
        job.crew.access.unreachable.len(),
        job.crew.deferrals.deferred.len(),
    ));
}

/// A long stall with ways in still held open: what holds them.
pub(super) fn held_ways_in(
    ctx: &Ctx,
    design: &Design,
    survey: &Survey,
    crew: &Crew,
    closing: bool,
) {
    if !TRACE || closing || !ctx.now.is_multiple_of(400) || ctx.now <= crew.pace.progress_at + 2000
    {
        return;
    }
    let holding: Vec<String> = survey
        .known
        .iter()
        .enumerate()
        .skip(crew.pace.cursor)
        .filter(|(_, k)| matches!(k, Known::Place(_) | Known::Clear { .. }))
        .filter(|(i, _)| {
            !crew.deferrals.cutters.contains(i) && !crew.built.contains(i) && !design.passage(*i)
        })
        .take(8)
        .map(|(i, k)| {
            format!(
                "Unit({i}) at {:?} {} deferred {} floating {} unreachable {}",
                design.units[i].pos,
                if matches!(k, Known::Clear { .. }) {
                    "clear"
                } else {
                    "place"
                },
                crew.deferrals.deferred(Task::Unit(i), ctx.now),
                crew.faces.floating.contains(&i),
                crew.access.unreachable.contains(&i)
            )
        })
        .collect();
    log(&format!("TRACE stalled; ways in held for {holding:?}"));
}

/// Where a way in stands in the queue.
pub(super) fn passage(
    ctx: &Ctx,
    design: &Design,
    crew: &Crew,
    i: usize,
    known: &Known,
    carried: &BTreeMap<ItemKey, u32>,
    closing: bool,
) {
    if !TRACE || !ctx.now.is_multiple_of(400) || !design.passage(i) {
        return;
    }
    let carried_ok = matches!(known, Known::Place(m) if cargo::holds(carried, m));
    log(&format!(
        "TRACE passage Unit({i}) at {:?}: deferred {} closing {closing} carried {carried_ok} built {}",
        design.units[i].pos,
        crew.deferrals.deferred(Task::Unit(i), ctx.now),
        crew.built.contains(&i)
    ));
}

/// What stands between the golem and the task its climb was planned for.
/// Asks the verdict again, so it is never called but when tracing.
pub(super) fn committed(ctx: &mut Ctx, job: &mut Job, body: &Body, task: Task) {
    let cells = task_cells(job, task);
    let known = match (task, job.survey.as_ref()) {
        (Task::Unit(i), Some(s)) => {
            format!("{:?} built {}", s.known[i], job.crew.built.contains(&i))
        }
        _ => "-".into(),
    };
    let verdict = verdict_name(viability(ctx, job, body, task, body.pos));
    let check = match task {
        Task::Unit(i) => {
            let unit = job.design.units[i];
            let record = job.design.records[unit.record as usize].clone();
            format!(
                "{:?}",
                actor_place_check(body.actor(), body.pos, unit.pos, record, true)
            )
        }
        _ => "-".into(),
    };
    log(&format!(
        "TRACE committed {task:?} cells {cells:?} from {:?} pos {:?}: {known}; reaches {} sees {} verdict {verdict} check {check} tried {}",
        body.cell,
        body.pos,
        reaches(body.pos, &cells),
        sees_from_here(job, task, body, &cells),
        job.crew.deferrals.tried.contains(&task.unit())
    ));
}

/// From a pillar's top: the open work near it, and why none of it is in hand.
pub(super) fn perch_top(job: &Job, body: &Body, top: [i32; 3], candidates: &[(Task, [i32; 3])]) {
    if !TRACE || body.cell != top {
        return;
    }
    let near: Vec<String> = candidates
        .iter()
        .filter(|(_, c)| crate::geometry::manhattan(*c, body.cell) <= 6)
        .map(|(task, c)| {
            let cells = task_cells(job, *task);
            format!(
                "{c:?} blind {} reaches {} sees {}",
                job.crew.deferrals.blind(*task, body.cell),
                reaches(body.pos, &cells),
                sees_from_here(job, *task, body, &cells)
            )
        })
        .collect();
    log(&format!(
        "TRACE perch top {top:?} candidates near: {near:?}"
    ));
}

/// Work is left and all of it waits: the nearest of it, and until when.
pub(super) fn deferred(ctx: &Ctx, job: &Job, candidates: &[(Task, [i32; 3])]) {
    if !TRACE || !ctx.now.is_multiple_of(200) {
        return;
    }
    let top: Vec<String> = candidates
        .iter()
        .take(6)
        .map(|(t, c)| format!("{t:?}@{c:?} until {:?}", job.crew.deferrals.deferred.get(t)))
        .collect();
    log(&format!(
        "TRACE deferred candidates at {}: {top:?}",
        ctx.now
    ));
}
