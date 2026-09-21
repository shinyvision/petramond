//! Asking the world whether a task would be accepted before anyone is sent
//! to do it, and acting on the answer.

use mod_sdk::*;

use super::{defer_task, places};
use crate::geometry::{offset, FACES};
use crate::jobs::Job;
use crate::survey::Known;
use crate::worker::crew::Deferrals;
use crate::worker::tuning::patience::{BURY_ROUNDS, DUE_PATIENCE};
use crate::worker::tuning::waits::{BURY_WAIT, REFUSED, SEALED_WAIT, UNLOADED, UNREAD};
use crate::worker::tuning::window::DUE_CHAIN;
use crate::worker::upkeep::open_block;
use crate::worker::{pocket, Body, Ctx, Task};

/// What the world answers a planned task, asked before the golem is sent:
/// the planner hands out only work that will be accepted.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Viable {
    /// Accepted from where the golem stands now.
    Now,
    /// Only reach or sight stand in the way: another stance may do.
    Elsewhere,
    /// Refused for a reason no stance changes; asked again after this long.
    Waits(u64),
    /// Nothing left to do there.
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
    // Laid now, it would close the last way in to work beside it; landing
    // that work wakes it. A unit taken down to open a way in stays down until
    // what it opened for stands.
    if job.crew.access.reopen.values().flatten().any(|o| *o == i) {
        return Viable::Waits(SEALED_WAIT);
    }
    // Laid now, it would bury earth still to be dug that only its cell lays
    // bare: the digging goes first, but not forever, so work nothing reaches
    // does not hold its neighbours back.
    if buries(ctx, job, i) && Deferrals::round(&mut job.crew.deferrals.bury_waits, i, BURY_ROUNDS) {
        return Viable::Waits(BURY_WAIT);
    }
    match pocket::seals(ctx, job, i) {
        Some(None) => {}
        Some(Some(sealed)) => {
            // A window it would close in is glazed now, not last: behind the
            // wall it could never be seen again.
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
            // Reach and sight are judged before faces: from afar a block with
            // nothing to hang on reads as merely out of reach, and a trip or a
            // climb would be made to hear the rest.
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

/// Whether laying unit `i` would cover the last open face of a block beside
/// it that is still to be dug away.
fn buries(ctx: &mut Ctx, job: &Job, i: usize) -> bool {
    let Some(survey) = job.survey.as_ref() else {
        return false;
    };
    let cells = job.design.cells(job.design.units[i]);
    for cell in &cells {
        for face in FACES {
            let n = offset(*cell, face);
            if cells.contains(&n) {
                continue;
            }
            let to_dig = job.design.unit_at(n).is_some_and(|j| {
                matches!(
                    survey.known[j],
                    Known::Clear {
                        at,
                        holds_items: false,
                        ..
                    } if at == n
                )
            });
            if !to_dig {
                continue;
            }
            let around: Vec<[i32; 3]> = FACES
                .iter()
                .map(|f| offset(n, *f))
                .filter(|c| !cells.contains(c))
                .collect();
            let bare = get_blocks(around)
                .into_iter()
                .any(|b| b.is_some_and(|b| open_block(ctx, b)));
            if !bare {
                return true;
            }
        }
    }
    false
}

/// Nothing to hang on yet: a neighbour landing usually brings one; a cell
/// that stays faceless gets scaffolding under it.
pub(super) fn faceless(ctx: &mut Ctx, job: &mut Job, i: usize) -> Viable {
    // Due work leading here from something standing is the face it waits for:
    // those blocks are owed anyway, and landing one wakes it. Not forever: a
    // chain whose first block never goes in must not hold the rest back.
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

/// Whether unit `i`, with nothing to be placed against, is joined through
/// blocks still to be laid to one that has: the build itself will bring it
/// a face.
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

/// Whether anything beside `unit`'s cells gives a placement a face: a block
/// no placement could take. `None` while a neighbour is unreadable.
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

/// Whether the world would still refuse `task` from `stance` for a reason
/// the trip there cannot fix (nothing to hang on yet): the verdict is
/// settled instead of walking or climbing there to hear it.
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

/// Act on a verdict that sends nobody: wait it out or forget done work.
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
