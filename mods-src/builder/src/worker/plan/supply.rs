use super::{Flow, Round};
use crate::project::Projects;
use crate::worker::crew::Stamped;
use crate::worker::tuning::waits::TOOL_TRIP_WAIT;
use crate::worker::waiting::Waiting;
use crate::worker::Job;
use crate::worker::{cargo, scaffold, Body, Ctx};

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
