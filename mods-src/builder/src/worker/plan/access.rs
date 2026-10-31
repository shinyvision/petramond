use super::{Flow, Mode, Round};
use crate::project::Projects;
use crate::worker::tuning::window::WINDOW;
use crate::worker::waiting::Waiting;
use crate::worker::{pocket, Body, Ctx, Job};

pub(super) fn survey(
    ctx: &mut Ctx,
    _projects: &mut Projects,
    job: &mut Job,
    _body: &Body,
    round: &mut Round,
) -> Flow {
    if round.mode != Mode::Building || pocket::refresh(ctx, job, &round.project) {
        Flow::Pass
    } else {
        Flow::Busy(Waiting::AccessGraph)
    }
}

pub(super) fn recover(
    ctx: &mut Ctx,
    _projects: &mut Projects,
    job: &mut Job,
    _body: &Body,
    round: &mut Round,
) -> Flow {
    let score = job.access_plan.score().unwrap_or(0);
    if round.mode != Mode::Building || score == 0 {
        return Flow::Pass;
    }
    round.mode = Mode::Unsealing;
    round.candidates = job.access_plan.candidates();
    round
        .candidates
        .retain(|(task, _)| !job.crew.deferrals.deferred(*task, ctx.now));
    if ctx.now.is_multiple_of(400) {
        trace!(
            "TRACE unsealing: {score} sealed, candidates {:?}, blocked units {:?}",
            round.candidates,
            job.access_plan
                .sealed_units()
                .iter()
                .map(|i| (*i, job.design.units[*i].pos))
                .collect::<Vec<_>>()
        );
    }
    round.candidates.truncate(WINDOW);
    round.future.clear();
    round.wants_items = false;
    round.waiting = true;
    job.crew.note = format!("Opening access to {score} sealed blocks");
    Flow::Pass
}
