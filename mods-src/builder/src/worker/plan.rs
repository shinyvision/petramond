use crate::host::prelude::*;

use super::body::Body;
use super::step::{Step, Task, Then};
use super::waiting::Waiting;
use super::Ctx;
use crate::project::{Note, Phase, Project, Projects};
use crate::survey::Known;
use crate::worker::Job;
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
    StrikingScaffold,
    GoingHome,
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
}

type Stage = fn(&mut Ctx, &mut Projects, &mut Job, &Body, &mut Round) -> Flow;

const STAGES: [Stage; 22] = [
    upkeep::reconcile_scaffolds,
    upkeep::check_home,
    upkeep::grounded,
    upkeep::rest,
    upkeep::recover_perch,
    upkeep::way_out,
    home::go_home,
    upkeep::surveyed,
    supply::full_hands,
    gather::weigh_work,
    perch::aloft,
    supply::scaffold_blocks,
    supply::tools,
    here::from_here,
    stances::rank_by_walk,
    stances::stance_spots,
    stances::nearest_spot,
    climb::pillars,
    unreached::finish_way_in,
    supply::resupply,
    unreached::dig_on,
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
    };
    for stage in STAGES {
        match stage(ctx, projects, job, body, &mut round) {
            Flow::Go(step) => return step,
            Flow::Busy(why) => {
                job.crew.why(why);
                return Step::Plan;
            }
            Flow::Pass => {}
        }
    }
    finish(projects, job)
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

fn finish(projects: &mut Projects, job: &mut Job) -> Step {
    let lost = match job.survey.as_mut() {
        Some(survey) => {
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
            open.iter().filter(|i| survey.known[**i].open()).count()
        }
        None => 0,
    };
    let report = Note::report(lost as u32, job.crew.scaffolding.left_standing);
    projects.update(job.id, |p| p.wind_down(report));
    job.crew.note.clear();
    Step::Plan
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
