use crate::host::prelude::*;

use super::{cargo, sight, Body, Ctx};
use crate::worker::Job;

const MINE: &str = "mine";

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Dig {
    Turning,
    Digging,
    Breaking,
    Nothing,
    Refused(ActionRefusal),
}

pub fn dig(ctx: &mut Ctx, job: &mut Job, body: &Body, cell: [i32; 3], collect: bool) -> Dig {
    let Some(block) = get_block(cell) else {
        return rest(job, body, Dig::Refused(ActionRefusal::Unloaded));
    };
    let tool = cargo::tool_slot(ctx, &body.slots, block);
    job.crew.presence.set_hold(body.id, true);
    if let Err(refusal) = sight::turn(ctx, job, body, &sight::block(cell)) {
        return rest(job, body, Dig::Refused(refusal));
    }
    job.crew.presence.hold_item(
        body.id,
        tool.and_then(|s| body.slots[s as usize].as_ref().map(|t| t.item.clone())),
    );
    job.crew.presence.animate(body.id, Some(MINE));
    match actor_dig(body.actor(), cell, tool, collect) {
        DigProgress::Refused(ActionRefusal::NotAimed) if sight::still_turning(job) => Dig::Turning,
        DigProgress::Digging { .. } => {
            job.crew.presence.unaimed = 0;
            Dig::Digging
        }
        DigProgress::Breaking => rest(job, body, Dig::Breaking),
        DigProgress::Refused(ActionRefusal::NothingToDo) => rest(job, body, Dig::Nothing),
        DigProgress::Refused(refusal) => rest(job, body, Dig::Refused(refusal)),
    }
}

fn rest(job: &mut Job, body: &Body, outcome: Dig) -> Dig {
    job.crew.presence.unaimed = 0;
    job.crew.presence.animate(body.id, None);
    outcome
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Lay {
    Turning,
    Queued,
    Satisfied,
    Refused(ActionRefusal),
}

pub fn lay(
    ctx: &mut Ctx,
    job: &mut Job,
    body: &Body,
    cell: [i32; 3],
    record: BlockRecord,
    pay: bool,
    shown: Option<String>,
) -> Lay {
    job.crew.presence.set_hold(body.id, true);
    let work = sight::Work::Click {
        pos: cell,
        record: Some(record.clone()),
    };
    if let Err(refusal) = sight::turn(ctx, job, body, &work) {
        job.crew.presence.unaimed = 0;
        return Lay::Refused(refusal);
    }
    job.crew.presence.hold_item(body.id, shown);
    match actor_place(body.actor(), cell, record, pay) {
        PlaceRequest::Refused(ActionRefusal::NotAimed) if sight::still_turning(job) => Lay::Turning,
        PlaceRequest::Queued => {
            job.crew.presence.unaimed = 0;
            job.crew.pace.placed_at = ctx.now;
            job.crew.presence.jab(body.id);
            Lay::Queued
        }
        PlaceRequest::Satisfied => Lay::Satisfied,
        PlaceRequest::Refused(refusal) => {
            job.crew.presence.unaimed = 0;
            Lay::Refused(refusal)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Use {
    Turning,
    Done,
    Refused,
}

pub fn use_block(ctx: &mut Ctx, job: &mut Job, body: &Body, cell: [i32; 3]) -> Use {
    job.crew.presence.set_hold(body.id, true);
    if sight::turn(ctx, job, body, &sight::block(cell)).is_err() {
        return Use::Refused;
    }
    if actor_interact(body.actor(), cell) {
        job.crew.presence.unaimed = 0;
        job.crew.presence.jab(body.id);
        return Use::Done;
    }
    if sight::still_turning(job) {
        Use::Turning
    } else {
        job.crew.presence.unaimed = 0;
        Use::Refused
    }
}
