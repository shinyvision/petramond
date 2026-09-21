//! Where the golem looks. Its hands work only what its eyes rest on: a block
//! goes in against a face under its gaze, turned the way it is looking, and
//! a dig or a door answers to a look at the block itself. So a place to
//! stand is one with such a look, and every action waits for the head.

use mod_sdk::*;

use super::tuning::patience::GAZE_TRIES;
use super::{Body, Ctx, Task};
use crate::geometry::feet_of;
use crate::jobs::Job;
use crate::survey::Known;

/// What a look must find for a piece of work.
#[derive(Clone)]
pub enum Work {
    /// Any of the cells under the crosshair, as a click on them would
    /// need: a chest opened.
    Touch,
    /// A click: `record` built at `pos`, or without one the block there dug
    /// or used.
    Click {
        pos: [i32; 3],
        record: Option<BlockRecord>,
    },
}

/// A scaffold block stood in `pos`, of whatever the golem has to hand.
pub fn scaffold(job: &Job, pos: [i32; 3]) -> Work {
    match job.crew.scaffolding.block.clone() {
        Some(record) => Work::Click {
            pos,
            record: Some(record),
        },
        None => Work::Touch,
    }
}

pub fn block(pos: [i32; 3]) -> Work {
    Work::Click { pos, record: None }
}

pub fn work(job: &Job, task: Task) -> Work {
    match task {
        Task::Unit(i) => {
            let unit = job.design.units[i];
            match job.survey.as_ref().map(|s| &s.known[i]) {
                Some(Known::Clear { at, .. }) => block(*at),
                _ => Work::Click {
                    pos: unit.pos,
                    record: Some(job.design.records[unit.record as usize].clone()),
                },
            }
        }
        Task::Support { cell, .. } => scaffold(job, cell),
        Task::Scaffold(cell) | Task::Trim(cell) | Task::Breakout(cell) => block(cell),
        Task::Reopen(o) => block(job.design.units[o].pos),
    }
}

/// Whether the golem standing in each of `stances` would have the look
/// `work` needs at `cells`: `None` = it would.
pub fn sees(
    body: &Body,
    stances: &[[i32; 3]],
    cells: &[[i32; 3]],
    work: &Work,
) -> Vec<Option<ActionRefusal>> {
    sees_from(
        body,
        stances.iter().map(|s| feet_of(*s)).collect(),
        cells,
        work,
    )
}

pub fn sees_from(
    body: &Body,
    from: Vec<[f64; 3]>,
    cells: &[[i32; 3]],
    work: &Work,
) -> Vec<Option<ActionRefusal>> {
    match work {
        Work::Touch => clicks_any(body, from, cells),
        Work::Click { pos, record } => actor_aims(body.actor(), from, *pos, record.clone())
            .into_iter()
            .map(Result::err)
            .collect(),
    }
}

/// Whether a click on any of `cells` would land from each of `from`, by the
/// rule the engine judges a dig or a use with: `None` = one would.
pub fn clicks_any(
    body: &Body,
    from: Vec<[f64; 3]>,
    cells: &[[i32; 3]],
) -> Vec<Option<ActionRefusal>> {
    let mut refusals: Vec<Option<ActionRefusal>> =
        vec![Some(ActionRefusal::NothingToDo); from.len()];
    for cell in cells {
        let aims = actor_aims(body.actor(), from.clone(), *cell, None);
        for (refusal, aim) in refusals.iter_mut().zip(aims) {
            if refusal.is_some() {
                *refusal = aim.err();
            }
        }
    }
    refusals
}

/// Turn the head (and the body after it) to `work` from where the golem
/// stands. `Err` = nothing to look at from here.
pub fn turn(ctx: &Ctx, job: &mut Job, body: &Body, work: &Work) -> Result<(), ActionRefusal> {
    let Work::Click { pos, record } = work else {
        return Ok(());
    };
    let aim = actor_aims(body.actor(), vec![body.pos], *pos, record.clone())
        .into_iter()
        .next()
        .unwrap_or(Err(ActionRefusal::NoActor))?;
    job.crew.presence.look_at(body.id, ctx.now, body.pos, aim);
    Ok(())
}

/// Whether an action refused as not aimed at should simply be asked again:
/// the head is still coming round. Past [`GAZE_TRIES`] the look is not coming.
pub fn still_turning(job: &mut Job) -> bool {
    job.crew.presence.unaimed += 1;
    job.crew.presence.unaimed <= GAZE_TRIES
}
