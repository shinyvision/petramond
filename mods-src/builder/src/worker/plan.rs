use crate::host::prelude::*;

use super::body::Body;
use super::step::{Step, Task, Then};
use super::waiting::Waiting;
use super::Ctx;
use crate::geometry::offset;
use crate::project::{Hold, Note, Phase, Project, Projects};
use crate::survey::Known;
use crate::worker::route;
use crate::worker::tuning::patience::{BUSY_PATIENCE, REGROW_PATIENCE, UNBUILT_PATIENCE};
use crate::worker::Job;
use crate::worker::{BARE_BEHAVIOR, COVER_BEHAVIOR};
mod access;
mod choice;
mod climb;
mod gather;
mod glazing;
mod here;
mod home;
mod perch;
mod sealing;
mod stances;
mod supply;
mod trace;
mod unreached;
mod upkeep;
mod verdict;

pub(super) use here::{clear_of_body, sees_from_here};
pub(super) use sealing::{as_walls, cutting};
pub(super) use verdict::waits_there;

pub(super) enum Flow {
    Go(Step),
    Busy(Waiting),
    Pass,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    Building,
    Unsealing,
    StrikingScaffold,
    GoingHome,
}

/// The access graph only matters while there is building to do.
pub(super) fn building(project: &Project) -> bool {
    Mode::of(project) == Mode::Building
}

impl Mode {
    pub(super) fn of(project: &Project) -> Self {
        match (project.phase(), project.scaffolds.is_empty()) {
            (Phase::Returning, true) => Mode::GoingHome,
            (Phase::Returning, false) => Mode::StrikingScaffold,
            _ => Mode::Building,
        }
    }
}

struct Round {
    project: Project,
    mode: Mode,
    candidates: Vec<(Task, [i32; 3])>,
    wants_items: bool,
    waiting: bool,
    walks: Vec<Option<u32>>,
    wants_perch: Vec<(Task, bool)>,
    spots: Vec<stances::Spot>,
    future: Vec<(Task, [i32; 3])>,
    supplies: supply::Requests,
    /// The worker stands where no walk leads home: it works what it can get at from here, then leaves.
    stranded: bool,
}

type Stage = fn(&mut Ctx, &mut Projects, &mut Job, &Body, &mut Round) -> Flow;

const STAGES: [Stage; 24] = [
    upkeep::reconcile_scaffolds,
    upkeep::check_home,
    upkeep::grounded,
    upkeep::rest,
    upkeep::recover_perch,
    upkeep::way_out,
    home::go_home,
    upkeep::surveyed,
    access::survey,
    gather::weigh_work,
    access::recover,
    perch::aloft,
    supply::consider,
    here::from_here,
    stances::rank_by_walk,
    stances::stance_spots,
    stances::nearest_spot,
    climb::pillars,
    unreached::finish_way_in,
    supply::resupply,
    unreached::dig_on,
    unreached::make_room,
    upkeep::leave_stranded,
    waits,
];

pub fn plan(ctx: &mut Ctx, projects: &mut Projects, job: &mut Job, body: &Body) -> Step {
    let Some(project) = projects.get(job.id).cloned() else {
        return Step::Plan;
    };
    trace::heartbeat(ctx, job, body);
    let mut round = Round {
        mode: Mode::of(&project),
        project,
        candidates: Vec::new(),
        wants_items: false,
        waiting: false,
        walks: Vec::new(),
        wants_perch: Vec::new(),
        spots: Vec::new(),
        future: Vec::new(),
        supplies: supply::Requests::default(),
        stranded: false,
    };
    let step = plan_round(ctx, projects, job, body, &mut round);
    if step != Step::Plan {
        job.crew.pace.busy_since = None;
        return step;
    }
    let since = *job.crew.pace.busy_since.get_or_insert(ctx.now);
    if ctx.now > since + BUSY_PATIENCE {
        job.crew.pace.busy_since = None;
        return unbusy(ctx, job, body, &round.project);
    }
    step
}

fn plan_round(
    ctx: &mut Ctx,
    projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    round: &mut Round,
) -> Step {
    for stage in STAGES {
        match stage(ctx, projects, job, body, round) {
            Flow::Go(step) => return step,
            Flow::Busy(why) => {
                job.crew.why(why);
                return Step::Plan;
            }
            Flow::Pass => {}
        }
    }
    finish(ctx, projects, job, body)
}

fn waits(
    ctx: &mut Ctx,
    _projects: &mut Projects,
    job: &mut Job,
    _body: &Body,
    round: &mut Round,
) -> Flow {
    let candidates = &round.candidates;
    if !candidates.is_empty() {
        trace::deferred(ctx, job, candidates);
        return Flow::Busy(Waiting::CandidatesDeferred);
    }
    if round.waiting {
        return Flow::Busy(Waiting::DeferredOrUnloaded);
    }
    Flow::Pass
}

pub(super) fn places(job: &Job, task: Task) -> bool {
    matches!(task, Task::Unit(i) if matches!(job.survey.as_ref().map(|s| &s.known[i]), Some(Known::Place(_))))
}

fn finish(ctx: &mut Ctx, projects: &mut Projects, job: &mut Job, body: &Body) -> Step {
    let Some(project) = projects.get(job.id).cloned() else {
        return Step::Plan;
    };
    // Winding down with scaffolding nothing here offers to strike (it stands under the worker, or
    // out of its reach): get home first, where every column is struck from the ground; what
    // still stands from there stays standing.
    if project.phase() == Phase::Returning && !project.scaffolds.is_empty() {
        if job.crew.aloft.perch.is_some() {
            return Step::Descend { since: ctx.now };
        }
        if body.cell != project.home {
            let hubs = route::Hubs::new(project.home, &job.crew.trail);
            return match route::out(ctx, hubs, body.cell, &[]) {
                None => Step::Plan,
                Some(Route::Open) => walk_to(ctx, project.home, Then::Regroup),
                Some(_) => super::rescue::rescue(ctx, job, body, &project),
            };
        }
        job.crew.scaffolding.left_standing += project.scaffolds.len() as u32;
        projects.update(job.id, |p| p.scaffolds.clear());
    }
    if regrowing(ctx, job) {
        job.crew.note = "Waiting for the ground cover to grow back".into();
        return Step::Plan;
    }
    if project.phase() == Phase::Working && short_of_unbuilt(ctx, projects, job, body, &project) {
        return Step::Plan;
    }
    // Work still open that nothing offered this round is not done: the last stages get their
    // time with it, and only what is still open after that is reported unbuilt.
    let (unbuilt, open_still) = if project.phase() == Phase::Working {
        unbuilt(ctx, job, &project)
    } else {
        (0, 0)
    };
    if open_still == 0 {
        job.crew.pace.unbuilt_since = None;
    } else if ctx.now < *job.crew.pace.unbuilt_since.get_or_insert(ctx.now) + UNBUILT_PATIENCE {
        job.crew.note = "Working out how to reach the last blocks".into();
        return Step::Plan;
    }
    let lost = lost(job);
    let report = Note::report(lost, job.crew.scaffolding.left_standing, unbuilt);
    projects.update(job.id, |p| p.wind_down(report));
    job.crew.note.clear();
    Step::Plan
}

/// Ground cover (grass) the work smothered turns back into its bare block (dirt), which takes the
/// cover again from a neighbour on its own. Digging the bare block only leaves a hole, since
/// nothing else gives the cover back.
pub(in crate::worker) fn regrows(
    ctx: &mut Ctx,
    design: &crate::design::Design,
    i: usize,
    known: &Known,
) -> bool {
    let Known::Clear { block, .. } = known else {
        return false;
    };
    let record = &design.records[design.units[i].record as usize];
    let behaves = |info: Option<&BlockInfoData>, key: &str| {
        info.is_some_and(|info| info.behaviors.iter().any(|b| b == key))
    };
    behaves(ctx.caches.block_named(&record.block), COVER_BEHAVIOR)
        && behaves(ctx.caches.block(*block), BARE_BEHAVIOR)
}

/// Planning has led nowhere for a long time: the questions asked from here never get an answer,
/// so the worker changes where it asks from, walking home or getting out.
fn unbusy(ctx: &mut Ctx, job: &mut Job, body: &Body, project: &Project) -> Step {
    trace!("TRACE planning at {:?} got no answers: moving", body.cell);
    if job.crew.aloft.perch.is_some() {
        return Step::Descend { since: ctx.now };
    }
    let hubs = route::Hubs::new(project.home, &job.crew.trail);
    match route::out(ctx, hubs, body.cell, &[]) {
        Some(Route::Open) if body.cell != project.home => walk_to(ctx, project.home, Then::Regroup),
        Some(Route::Open) => Step::Plan,
        _ => super::rescue::rescue(ctx, job, body, project),
    }
}

/// Finished units that came undone and stayed undone.
pub(super) fn lost(job: &mut Job) -> u32 {
    let Some(survey) = job.survey.as_mut() else {
        return 0;
    };
    let open: Vec<usize> = job
        .crew
        .built
        .iter()
        .copied()
        .filter(|i| survey.known[*i].open())
        .collect();
    survey.measure(&job.design, &open);
    if super::TRACE {
        for i in open.iter().filter(|i| survey.known[**i].open()) {
            let unit = job.design.units[*i];
            log(&format!(
                "TRACE lost Unit({i}) {} at {:?}: {:?}",
                job.design.records[unit.record as usize].block, unit.pos, survey.known[*i]
            ));
        }
    }
    open.iter().filter(|i| survey.known[**i].open()).count() as u32
}

/// Open work the worker never finished, leaving out its own scaffolding and ground cover growing back.
/// Counted twice: all of it, and the part not yet given up on.
fn unbuilt(ctx: &mut Ctx, job: &Job, project: &Project) -> (u32, u32) {
    let Some(survey) = job.survey.as_ref() else {
        return (0, 0);
    };
    let (mut all, mut open_still) = (0, 0);
    for (i, known) in survey.known.iter().enumerate() {
        let open = match known {
            Known::Place(_) => true,
            Known::Clear { at, .. } => !project.scaffolds.contains(at),
            _ => false,
        };
        if open && !job.crew.built.contains(&i) && !regrows(ctx, &job.design, i, known) {
            all += 1;
            if !job.crew.access.given_up.contains(&i) {
                open_still += 1;
            }
        }
    }
    (all, open_still)
}

/// Work never built whose blocks are neither in hand nor in the chests is asked for, as at the
/// start, rather than left out of a finished job without a word.
fn short_of_unbuilt(
    ctx: &mut Ctx,
    projects: &mut Projects,
    job: &Job,
    body: &Body,
    project: &Project,
) -> bool {
    let Some(survey) = job.survey.as_ref() else {
        return false;
    };
    let mut bill: std::collections::BTreeMap<crate::survey::ItemKey, u32> = Default::default();
    for (i, known) in survey.known.iter().enumerate() {
        if let Known::Place(missing) = known {
            if !job.crew.built.contains(&i) {
                for stack in missing {
                    *bill.entry(crate::survey::key_of(stack)).or_default() +=
                        u32::from(stack.count);
                }
            }
        }
    }
    if bill.is_empty() {
        return false;
    }
    let stock = ctx.supplies.stock(project.table);
    if !stock.read {
        return true;
    }
    let mut have = stock.totals.clone();
    crate::supplies::add_totals(&mut have, &body.slots);
    let short = crate::supplies::shortfall(&bill, &have);
    let Some(worst) = crate::supplies::worst(&short, ctx.caches) else {
        return false;
    };
    projects.update(job.id, |p| p.hold_for(Hold::Supplies, Note::Missing(worst)));
    true
}

/// A finished job waits a while for ground cover still growing back before it calls it lost.
pub(super) fn regrowing(ctx: &mut Ctx, job: &mut Job) -> bool {
    let Some(survey) = job.survey.as_mut() else {
        return false;
    };
    let open: Vec<usize> = (0..survey.known.len())
        .filter(|i| matches!(survey.known[*i], Known::Clear { .. }))
        .collect();
    survey.measure(&job.design, &open);
    // Cover still under the worker's scaffolding cannot take yet; its time starts once uncovered.
    let scaffolds = &job.crew.scaffolding.cells;
    let any = open.into_iter().any(|i| {
        !scaffolds.contains(&offset(job.design.units[i].pos, [0, 1, 0]))
            && regrows(ctx, &job.design, i, &survey.known[i])
    });
    if !any {
        job.crew.pace.regrow_since = None;
        return false;
    }
    let since = *job.crew.pace.regrow_since.get_or_insert(ctx.now);
    ctx.now < since + REGROW_PATIENCE
}

pub(super) fn walk_to(ctx: &Ctx, to: [i32; 3], then: Then) -> Step {
    walk_via(ctx, to, to, then)
}

fn walk_via(ctx: &Ctx, to: [i32; 3], goal: [i32; 3], then: Then) -> Step {
    Step::Walk {
        to,
        goal,
        then,
        best: i32::MAX,
        progress: ctx.now,
    }
}

pub fn task_cells(job: &Job, task: Task) -> Vec<[i32; 3]> {
    match task {
        Task::Scaffold(cell) | Task::Support { cell, .. } => vec![cell],
        Task::Reopen(o) => job.design.cells(job.design.units[o]),
        Task::Trim(c) | Task::Breakout(c) => vec![c],
        Task::Unit(i) => match job.survey.as_ref().map(|s| &s.known[i]) {
            Some(Known::Clear { at, .. }) => vec![*at],
            _ => job.design.cells(job.design.units[i]),
        },
    }
}

pub(super) fn defer_task(ctx: &Ctx, job: &mut Job, task: Task, ticks: u64) {
    job.crew.deferrals.defer(task, ctx.now + ticks);
}

#[cfg(test)]
mod tests;
