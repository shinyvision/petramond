use crate::host::prelude::*;

use super::tuning::patience::GAZE_TRIES;
use super::{Body, Ctx, Task};
use crate::geometry::feet_of;
use crate::survey::Known;
use crate::worker::Job;

#[derive(Clone)]
pub enum Work {
    Touch,
    Click {
        pos: [i32; 3],
        record: Option<BlockRecord>,
    },
}

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

pub fn still_turning(job: &mut Job) -> bool {
    job.crew.presence.unaimed += 1;
    job.crew.presence.unaimed <= GAZE_TRIES
}
