//! The way home once the work is done.

use mod_sdk::*;

use super::{walk_to, walk_via, Flow, Mode, Round};
use crate::geometry::offset;
use crate::jobs::Job;
use crate::project::Projects;
use crate::worker::route::{self, Hubs};
use crate::worker::{cargo, sight, stance, Body, Ctx, Step, Then};

pub(super) fn returning(
    ctx: &mut Ctx,
    projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    project: &crate::project::Project,
) -> Step {
    if job.crew.aloft.perch.is_some() {
        return Step::Descend { since: ctx.now };
    }
    if !job.crew.cargo.chests_full && cargo::returns_anything(ctx, job, body, project) {
        return match cargo::unload_all(ctx, job, body, project) {
            Some(step) => step,
            None => Step::Plan,
        };
    }
    // Doors it propped open are shut behind it, from the home side.
    while let Some(&door) = job.crew.access.opened.first() {
        let trail = job.crew.trail.clone();
        let hubs = Hubs::new(project.home, &trail);
        let shut = [door, offset(door, [0, 1, 0])];
        // Judged on the upper half: the lower one hides behind it.
        let stance = match stance::find(
            ctx,
            body,
            hubs,
            &[shut[1]],
            &sight::block(shut[1]),
            false,
            |s| !shut.contains(&s),
        ) {
            stance::Search::Found(to) => to,
            stance::Search::Busy => return Step::Plan,
            stance::Search::None | stance::Search::Unseen => {
                trace!("TRACE door {door:?}: no stance to shut it from");
                job.crew.access.opened.remove(0);
                continue;
            }
        };
        match route::out(ctx, hubs, stance, &shut) {
            Some(Route::Open) => {}
            None => return Step::Plan,
            Some(_) => {
                // Only from inside: left open rather than shut in.
                trace!("TRACE door {door:?}: only shut from inside {stance:?}");
                job.crew.access.opened.remove(0);
                continue;
            }
        }
        return match route::leg(ctx, hubs, body, stance) {
            Some(Some(cell)) => walk_via(ctx, cell, stance, Then::Shut(door)),
            Some(None) => {
                job.crew.access.opened.remove(0);
                Step::Plan
            }
            None => Step::Plan,
        };
    }
    let home = project.home;
    if body.cell == home {
        projects.update(job.id, crate::project::Project::burrow);
        return Step::Burrow { t: 0 };
    }
    walk_to(ctx, home, Then::Home)
}

/// The work done and the scaffolding down: home.
pub(super) fn go_home(
    ctx: &mut Ctx,
    projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    round: &mut Round,
) -> Flow {
    if round.mode == Mode::GoingHome {
        return Flow::Go(returning(ctx, projects, job, body, &round.project));
    }
    Flow::Pass
}
