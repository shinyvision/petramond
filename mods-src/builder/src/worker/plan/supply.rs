use std::collections::BTreeSet;

use super::{choice, Flow, Round};
use crate::project::Projects;
use crate::worker::crew::Stamped;
use crate::worker::tuning::waits::TOOL_TRIP_WAIT;
use crate::worker::waiting::Waiting;
use crate::worker::{cargo, scaffold, Body, Ctx, Job};

#[derive(Default)]
pub(super) struct Requests {
    pub unload: bool,
    pub scaffold: bool,
    pub tools: BTreeSet<String>,
    pub materials: bool,
}

impl Requests {
    pub fn fetch(&self) -> bool {
        self.materials || self.scaffold || !self.tools.is_empty()
    }

    fn chosen(&self, ctx: &Ctx, job: &mut Job) {
        if self.scaffold {
            job.crew.scaffolding.short = false;
        }
        if !self.tools.is_empty() {
            job.crew.cargo.tool_trip = Stamped {
                at: ctx.now,
                value: self.tools.clone(),
            };
        }
        job.crew.why(if self.scaffold {
            Waiting::FetchingScaffold
        } else if !self.tools.is_empty() {
            Waiting::FetchingTools
        } else {
            Waiting::Resupply
        });
    }
}

pub(super) fn consider(
    ctx: &mut Ctx,
    _projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    round: &mut Round,
) -> Flow {
    if body.slots.iter().all(Option::is_some) {
        let mut ready = Vec::new();
        for (task, cell) in std::mem::take(&mut round.candidates) {
            if choice::collects(job, task) {
                round.supplies.unload = true;
                round.future.push((task, cell));
            } else {
                ready.push((task, cell));
            }
        }
        round.candidates = ready;
    }
    scaffold_request(ctx, job, body, round);
    let tools = cargo::tools_waiting(ctx, job, body, &round.project);
    let tripped = &job.crew.cargo.tool_trip;
    if ctx.now >= tripped.at + TOOL_TRIP_WAIT || !tools.is_subset(&tripped.value) {
        round.supplies.tools = tools;
    }
    round.supplies.materials = round.wants_items;
    Flow::Pass
}

pub(super) fn choose(ctx: &Ctx, job: &mut Job, round: &Round, trip: cargo::Trip) -> Flow {
    round.supplies.chosen(ctx, job);
    Flow::Go(trip.step)
}

fn scaffold_request(ctx: &mut Ctx, job: &mut Job, body: &Body, round: &mut Round) {
    if !job.crew.scaffolding.short {
        return;
    }
    if scaffold::in_hand(ctx, job, &body.slots) >= job.crew.scaffolding.want.max(1) {
        job.crew.scaffolding.short = false;
        return;
    }
    let stock = ctx.supplies.stock(round.project.table);
    round.supplies.scaffold =
        stock.read && scaffold::to_fetch(ctx, job, &body.slots, &stock.totals).is_some();
}

pub(super) fn resupply(
    ctx: &mut Ctx,
    projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    round: &mut Round,
) -> Flow {
    scaffold_request(ctx, job, body, round);
    if round.supplies.fetch() {
        let step = cargo::resupply(
            ctx,
            projects,
            job,
            body,
            &round.project,
            round.waiting || !round.candidates.is_empty(),
        );
        if matches!(
            step,
            crate::worker::Step::Walk {
                then: crate::worker::Then::Fetch(_),
                ..
            }
        ) {
            round.supplies.chosen(ctx, job);
        }
        return Flow::Go(step);
    }
    if round.supplies.unload {
        if let Some(step) = cargo::unload_all(ctx, job, body, &round.project) {
            return Flow::Go(step);
        }
    }
    if job.crew.scaffolding.short {
        job.crew.note =
            "The golem needs blocks to stand on: dirt, cobblestone, logs or planks".into();
        return Flow::Busy(Waiting::FetchingScaffold);
    }
    Flow::Pass
}
