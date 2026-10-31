use crate::host::prelude::*;

use super::{walk_to, walk_via, Flow, Mode, Round};
use crate::geometry::offset;
use crate::project::Projects;
use crate::survey::Known;
use crate::worker::route::{self, Hubs};
use crate::worker::Job;
use crate::worker::{cargo, scaffold, sight, stance, Body, Ctx, Step, Then};

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
    let orphans = orphaned_scaffolds(ctx, job);
    if !orphans.is_empty() {
        trace!("TRACE scaffolding with no record: {orphans:?}");
        projects.update(job.id, |p| p.scaffolds.extend(orphans));
        return Step::Plan;
    }
    if !job.crew.cargo.chests_full && cargo::returns_anything(ctx, job, body, project) {
        return match cargo::unload_all(ctx, job, body, project) {
            Some(step) => step,
            None => Step::Plan,
        };
    }
    while let Some(&door) = job.crew.access.opened.first() {
        let trail = job.crew.trail.clone();
        let hubs = Hubs::new(project.home, &trail);
        let shut = [door, offset(door, [0, 1, 0])];
        let stance = match stance::find(
            ctx,
            body,
            hubs,
            &[shut[1]],
            &sight::block(shut[1]),
            stance::Accept {
                off_top: false,
                one_way: false,
            },
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
    if super::regrowing(ctx, job) {
        job.crew.note = "Waiting for the ground cover to grow back".into();
        if body.cell == home {
            return Step::Plan;
        }
        return walk_to(ctx, home, Then::Regroup);
    }
    if body.cell == home {
        // Ground cover counted lost at the wind-down may have grown back since.
        let lost = super::lost(job);
        projects.update(job.id, |p| {
            if let crate::project::Note::Report(report) = &mut p.note {
                report.lost = lost;
            } else if lost > 0 {
                p.note = crate::project::Note::report(lost, 0, 0);
            }
        });
        projects.update(job.id, crate::project::Project::burrow);
        return Step::Burrow { t: 0 };
    }
    walk_to(ctx, home, Then::Home)
}

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

/// Scaffolding the worker no longer keeps a record of, still standing where the design wants air:
/// it is struck like any other before the worker leaves.
fn orphaned_scaffolds(ctx: &mut Ctx, job: &Job) -> Vec<[i32; 3]> {
    let Some(survey) = job.survey.as_ref() else {
        return Vec::new();
    };
    let mut orphans = Vec::new();
    for (i, known) in survey.known.iter().enumerate() {
        let Known::Clear { at, block, .. } = known else {
            continue;
        };
        let unit = job.design.units[i];
        if *at == unit.pos
            && !job.crew.scaffolding.left.contains(at)
            && matches!(job.design.plan(unit), crate::design::Plan::Air)
            && scaffold::still_standing(ctx, *block)
        {
            orphans.push(*at);
        }
    }
    orphans
}
