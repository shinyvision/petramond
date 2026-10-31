use crate::host::prelude::*;

use super::{defer_task, places};
use crate::geometry::{offset, FACES};
use crate::survey::Known;
use crate::worker::tuning::patience::DUE_PATIENCE;
use crate::worker::tuning::waits::{REFUSED, SEALED_WAIT, UNLOADED, UNREAD};
use crate::worker::tuning::window::DUE_CHAIN;
use crate::worker::Job;
use crate::worker::{pocket, Body, Ctx, Task};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Viable {
    Now,
    Elsewhere,
    Waits(u64),
    Done,
}

pub(super) fn viability(
    ctx: &mut Ctx,
    job: &mut Job,
    body: &Body,
    task: Task,
    from: [f64; 3],
) -> Viable {
    let Task::Unit(i) = task else {
        return Viable::Now;
    };
    if !places(job, task) {
        return Viable::Now;
    }
    if job.access_plan.score().is_some_and(|n| n > 0) {
        return Viable::Waits(SEALED_WAIT);
    }
    match pocket::seals(ctx, job, i) {
        Some(None) => {}
        Some(Some(sealed)) => {
            if job.design.glazing(sealed) {
                job.crew.glazing.ahead.insert(sealed);
            }
            trace!(
                "TRACE Unit({i}) at {:?} would seal work in",
                job.design.units[i].pos
            );
            return Viable::Waits(SEALED_WAIT);
        }
        None => {
            trace!("TRACE Unit({i}) seal check unreadable");
            return Viable::Waits(UNREAD);
        }
    }
    let unit = job.design.units[i];
    let record = job.design.records[unit.record as usize].clone();
    let answer = actor_place_check(body.actor(), from, unit.pos, record, true);
    if answer == PlaceRequest::Refused(ActionRefusal::NoSupport) {
        job.crew.faces.unheld.insert(i);
    } else {
        job.crew.faces.unheld.remove(&i);
    }
    match answer {
        PlaceRequest::Queued => Viable::Now,
        PlaceRequest::Satisfied => Viable::Done,
        PlaceRequest::Refused(refusal) => match refusal {
            ActionRefusal::OutOfReach | ActionRefusal::NoLineOfSight
                if face_beside(ctx, job, unit) == Some(false) =>
            {
                faceless(ctx, job, i)
            }
            ActionRefusal::OutOfReach
            | ActionRefusal::NoLineOfSight
            | ActionRefusal::Misaligned
            | ActionRefusal::NotAimed
            | ActionRefusal::MissingItems => Viable::Elsewhere,
            ActionRefusal::NothingToDo => Viable::Done,
            ActionRefusal::NoFace => faceless(ctx, job, i),
            ActionRefusal::BodyInTheWay | ActionRefusal::Changed => Viable::Waits(UNREAD),
            ActionRefusal::Unloaded | ActionRefusal::NoSupport => Viable::Waits(UNLOADED),
            _ => Viable::Waits(REFUSED),
        },
    }
}

pub(super) fn faceless(ctx: &mut Ctx, job: &mut Job, i: usize) -> Viable {
    if ctx.now <= job.crew.pace.progress_at + DUE_PATIENCE && hangs_on_due_work(ctx, job, i) {
        return Viable::Waits(SEALED_WAIT);
    }
    let fragile = job.design.fragile(job.design.units[i]);
    let hangs = job.crew.faces.hangs.contains(&i);
    if !fragile && !hangs && job.crew.faces.faceless_try(i, ctx.now) {
        job.crew.faces.floating.insert(i);
        Viable::Waits(0)
    } else {
        Viable::Waits(SEALED_WAIT)
    }
}

fn hangs_on_due_work(ctx: &mut Ctx, job: &Job, i: usize) -> bool {
    let Some(survey) = job.survey.as_ref() else {
        return false;
    };
    let due = |j: usize| matches!(survey.known[j], Known::Place(_)) && !job.crew.built.contains(&j);
    let mut seen = vec![i];
    let mut at = 0;
    while at < seen.len() && seen.len() < DUE_CHAIN {
        let unit = job.design.units[seen[at]];
        at += 1;
        let cells = job.design.cells(unit);
        for cell in &cells {
            for face in FACES {
                let Some(j) = job.design.unit_at(offset(*cell, face)) else {
                    continue;
                };
                if seen.contains(&j) || !due(j) {
                    continue;
                }
                if face_beside(ctx, job, job.design.units[j]) == Some(true) {
                    return true;
                }
                seen.push(j);
            }
        }
    }
    false
}

fn face_beside(ctx: &mut Ctx, job: &Job, unit: crate::design::Unit) -> Option<bool> {
    let cells = job.design.cells(unit);
    let beside: Vec<[i32; 3]> = crate::geometry::beside(&cells)
        .filter(|n| !cells.contains(n))
        .collect();
    for block in get_blocks(beside) {
        if ctx
            .caches
            .block(block?)
            .is_some_and(|info| !info.replaceable)
        {
            return Some(true);
        }
    }
    Some(false)
}

pub(in crate::worker) fn waits_there(
    ctx: &mut Ctx,
    job: &mut Job,
    body: &Body,
    task: Task,
    stance: [i32; 3],
) -> bool {
    let verdict = viability(ctx, job, body, task, crate::geometry::feet_of(stance));
    if matches!(verdict, Viable::Now | Viable::Elsewhere) {
        return false;
    }
    settle_verdict(ctx, job, task, verdict);
    true
}

pub(super) fn verdict_name(verdict: Viable) -> String {
    match verdict {
        Viable::Now => "now".into(),
        Viable::Elsewhere => "elsewhere".into(),
        Viable::Waits(t) => format!("waits {t}"),
        Viable::Done => "done".into(),
    }
}

pub(super) fn settle_verdict(ctx: &Ctx, job: &mut Job, task: Task, verdict: Viable) {
    match verdict {
        Viable::Waits(0) | Viable::Now | Viable::Elsewhere => {}
        Viable::Waits(ticks) => defer_task(ctx, job, task, ticks),
        Viable::Done => {
            if let (Task::Unit(i), Some(survey)) = (task, job.survey.as_mut()) {
                survey.measure(&job.design, &[i]);
            }
        }
    }
}
