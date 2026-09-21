//! Before any work is weighed: the records, home and the golem itself are
//! put in order, and a golem with no way home gets out first.

use mod_sdk::*;

use super::{Flow, Mode, Round};
use crate::geometry::offset;
use crate::jobs::Job;
use crate::project::Projects;
use crate::worker::route::{self, Hubs};
use crate::worker::tuning::every::HOME_CHECK_EVERY;
use crate::worker::waiting::Waiting;
use crate::worker::{lifecycle, pillar, rescue, site, wayin, Body, Ctx, Step};

/// The project's scaffold records, made true to the world.
/// The world is the truth about scaffolding: a record whose cell holds no
/// scaffold (refused, or broken by a player) is dropped, or a climb planned
/// to take it down finds nothing and plans the same climb again.
pub(super) fn reconcile_scaffolds(
    ctx: &mut Ctx,
    projects: &mut Projects,
    job: &mut Job,
    _body: &Body,
    round: &mut Round,
) -> Flow {
    let project = &mut round.project;
    if !project.scaffolds.is_empty() {
        let blocks = get_blocks(project.scaffolds.clone());
        let gone: Vec<[i32; 3]> = project
            .scaffolds
            .iter()
            .zip(blocks)
            .filter(|(_, b)| b.is_some_and(|b| ctx.content.scaffold_kind(b).is_none()))
            .map(|(c, _)| *c)
            .collect();
        if !gone.is_empty() {
            trace!("TRACE scaffold records with no scaffold: {gone:?}");
            projects.update(job.id, |p| p.scaffolds.retain(|c| !gone.contains(c)));
            project.scaffolds.retain(|c| !gone.contains(c));
        }
    }
    round.mode = Mode::of(&round.project);
    Flow::Pass
}

/// Home, kept ground to stand on.
/// Every way the golem is judged to walk starts from home, so home must
/// still be ground to stand on: a chest set down on it closed every route.
pub(super) fn check_home(
    ctx: &mut Ctx,
    projects: &mut Projects,
    job: &mut Job,
    _body: &Body,
    round: &mut Round,
) -> Flow {
    let project = &mut round.project;
    if ctx.now >= job.crew.pace.home_checked + HOME_CHECK_EVERY {
        job.crew.pace.home_checked = ctx.now;
        if lifecycle::homes(ctx, project.table, &[project.home]).first() == Some(&false) {
            if let Some(open) = lifecycle::find_home(ctx, job, project.table) {
                trace!(
                    "TRACE home {:?} is no ground to stand on: moved to {open:?}",
                    project.home
                );
                projects.update(job.id, |p| p.home = open);
                project.home = open;
                ctx.site = site(job, open);
                ctx.routes.clear();
                ctx.regions.clear();
            }
        }
    }
    Flow::Pass
}

/// A body in the air plans nothing: where it stands is not yet known.
pub(super) fn grounded(
    _ctx: &mut Ctx,
    _projects: &mut Projects,
    _job: &mut Job,
    body: &Body,
    _round: &mut Round,
) -> Flow {
    if !body.on_ground {
        return Flow::Go(Step::Plan);
    }
    Flow::Pass
}

/// Between steps the golem holds nothing and plays nothing, and where it
/// stood is remembered as a way back.
pub(super) fn rest(
    _ctx: &mut Ctx,
    _projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    _round: &mut Round,
) -> Flow {
    job.crew
        .presence
        .set_hold(body.id, job.crew.aloft.perch.is_some());
    if job.crew.aloft.perch.is_none() {
        job.crew.trail.stood(body.cell);
    }
    job.crew.presence.hold_item(body.id, None);
    job.crew.presence.animate(body.id, None);
    Flow::Pass
}

/// A pillar the golem stands on without knowing it is climbed down; a
/// walkway whose pillar is gone comes down as strays.
/// A scaffold laid to rise out of a hole is no pillar to climb down.
pub(super) fn recover_perch(
    ctx: &mut Ctx,
    _projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    round: &mut Round,
) -> Flow {
    let project = &round.project;
    if job.crew.aloft.perch.is_none() && job.crew.rescue.stuck.is_none() {
        if let Some(pillar) = pillar::recover(ctx, project, body) {
            job.crew.aloft.perch = Some(pillar);
            return Flow::Go(Step::Descend { since: ctx.now });
        }
        // A walkway outlives no pillar (the golem fell or hopped off): what
        // is left of it comes down as stray scaffolding.
        if let Some(walkway) = job.crew.aloft.bridge.take() {
            for support in walkway.path.iter().map(|p| offset(*p, [0, -1, 0])) {
                if project.scaffolds.contains(&support)
                    && !job.crew.scaffolding.urgent.contains(&support)
                {
                    job.crew.scaffolding.urgent.push(support);
                }
            }
        }
    }
    Flow::Pass
}

/// Nowhere a route leads home from (walled in, down a hole, up on a ledge):
/// out first, whatever else waits.
pub(super) fn way_out(
    ctx: &mut Ctx,
    _projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    round: &mut Round,
) -> Flow {
    let project = &round.project;
    if job.crew.aloft.perch.is_none() {
        let trail = job.crew.trail.clone();
        let hubs = Hubs::new(project.home, &trail);
        match route::out(ctx, hubs, body.cell, &[]) {
            Some(Route::Closed) => {
                // Ground still loading reads as open air to routes and as
                // nothing to the rest: believed shut only once it is all in.
                if !rescue::site_settled(ctx, job) {
                    return Flow::Busy(Waiting::SiteLoading);
                }
                if let Some(step) = wayin::off_threshold(ctx, hubs, body) {
                    return Flow::Go(step);
                }
                return Flow::Go(rescue::rescue(ctx, job, body, project));
            }
            Some(Route::Open) => {
                job.crew.rescue.stuck = None;
                job.crew.rescue.site_loaded_since = None;
            }
            _ => {}
        }
    }
    Flow::Pass
}

/// Nothing is planned against a site not yet surveyed.
pub(super) fn surveyed(
    _ctx: &mut Ctx,
    _projects: &mut Projects,
    job: &mut Job,
    _body: &Body,
    _round: &mut Round,
) -> Flow {
    if job.summary().is_none() {
        return Flow::Go(Step::Plan);
    }
    Flow::Pass
}
