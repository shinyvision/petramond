//! The trips to the chests a plan calls for.

use super::{Flow, Round};
use crate::jobs::Job;
use crate::project::Projects;
use crate::worker::crew::Stamped;
use crate::worker::tuning::waits::TOOL_TRIP_WAIT;
use crate::worker::waiting::Waiting;
use crate::worker::{cargo, scaffold, Body, Ctx};

/// Only hands with no room left go back to the chests: a trip per dug
/// block of dirt while a slot or two stood free was a trip per block.
pub(super) fn full_hands(
    ctx: &mut Ctx,
    _projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    round: &mut Round,
) -> Flow {
    let project = &round.project;
    let free = body.slots.iter().filter(|s| s.is_none()).count();
    if free == 0 && job.crew.aloft.perch.is_none() {
        if let Some(step) = cargo::unload(ctx, job, body, project) {
            return Flow::Go(step);
        }
    }
    Flow::Pass
}

/// Blocks to stand on before a climb that lacked them: fetched when the
/// chests hold any, asked for on the table when they do not.
pub(super) fn scaffold_blocks(
    ctx: &mut Ctx,
    projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    round: &mut Round,
) -> Flow {
    let project = &round.project;
    if std::mem::take(&mut job.crew.scaffolding.short) {
        let stock = ctx.supplies.stock(project.table);
        if stock.read {
            if scaffold::to_fetch(ctx, job, &body.slots, &stock.totals).is_some() {
                job.crew.why(Waiting::FetchingScaffold);
                return Flow::Go(cargo::resupply(ctx, projects, job, body, project, true));
            }
            job.crew.note =
                "The golem needs blocks to stand on: dirt, cobblestone, logs or planks".into();
        }
    }
    Flow::Pass
}

/// Tools before digging, not bare hands while the tool sits in a chest; a
/// trip that fetched nothing is not repeated for the same tools for a while.
pub(super) fn tools(
    ctx: &mut Ctx,
    projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    round: &mut Round,
) -> Flow {
    let project = &round.project;
    let tools = cargo::tools_waiting(ctx, job, body, project);
    let Stamped {
        at: tripped,
        value: fetched,
    } = &job.crew.cargo.tool_trip;
    if !tools.is_empty() && (ctx.now >= tripped + TOOL_TRIP_WAIT || !tools.is_subset(fetched)) {
        job.crew.cargo.tool_trip = Stamped {
            at: ctx.now,
            value: tools,
        };
        job.crew.why(Waiting::FetchingTools);
        return Flow::Go(cargo::resupply(ctx, projects, job, body, project, true));
    }
    Flow::Pass
}

/// Items first: work that cannot be reached this tick must not keep the
/// golem from fetching what reachable work is waiting for.
pub(super) fn resupply(
    ctx: &mut Ctx,
    projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    round: &mut Round,
) -> Flow {
    let project = &round.project;
    let candidates = &round.candidates;
    if round.wants_items {
        job.crew.why(Waiting::Resupply);
        return Flow::Go(cargo::resupply(
            ctx,
            projects,
            job,
            body,
            project,
            round.waiting || !candidates.is_empty(),
        ));
    }
    Flow::Pass
}
