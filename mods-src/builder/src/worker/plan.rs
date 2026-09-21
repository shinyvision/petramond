//! Choosing the golem's next step: the lowest open work it can do with what
//! it carries, reached from where it stands, from a stance it can walk to,
//! or from the top of a scaffold pillar; otherwise fetching, unloading, or
//! finishing up.
//!
//! A plan is a list of stages tried in order; the first with an answer ends
//! the tick's planning.

use mod_sdk::*;

use super::body::Body;
use super::step::{Step, Task, Then};
use super::waiting::Waiting;
use super::Ctx;
use crate::jobs::Job;
use crate::project::{Phase, Project, Projects};
use crate::survey::Known;
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

/// What a stage of the plan comes to.
pub(super) enum Flow {
    /// The golem's next step.
    Go(Step),
    /// No answer this tick (a search ran out of budget, the world is still
    /// loading): plan again, saying what is waited on.
    Busy(Waiting),
    /// Nothing for this stage to do: on to the next.
    Pass,
}

/// What a job's golem is there to do. Only a building golem lays anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode {
    Building,
    /// The work is done and its scaffolding still stands: it comes down.
    StrikingScaffold,
    /// Nothing is left but the way home.
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

/// What one tick's plan has worked out so far, handed from stage to stage.
struct Round {
    project: Project,
    mode: Mode,
    /// The open work being weighed, in the order it is tried.
    candidates: Vec<(Task, [i32; 3])>,
    /// Whether any placement waits on items, and whether any work waits on time.
    wants_items: bool,
    waiting: bool,
    /// The walk to each candidate, in moves, where the site's flood gives one.
    walks: Vec<Option<u32>>,
    /// Candidates no stance was found for, and whether one was only unseen.
    wants_perch: Vec<(Task, bool)>,
    spots: Vec<stances::Spot>,
}

type Stage = fn(&mut Ctx, &mut Projects, &mut Job, &Body, &mut Round) -> Flow;

const STAGES: [Stage; 22] = [
    // Before the perch is recovered: recovery trusts the scaffold records.
    upkeep::reconcile_scaffolds,
    // Before the way out: every route is judged from home.
    upkeep::check_home,
    // Before anything reads where the body stands.
    upkeep::grounded,
    upkeep::rest,
    // After the records are true, before the way out: a golem on a pillar is
    // not stranded.
    upkeep::recover_perch,
    // Before going home and before any work: out first, whatever else waits.
    upkeep::way_out,
    home::go_home,
    upkeep::surveyed,
    // Before work is gathered: full hands lay nothing.
    supply::full_hands,
    gather::weigh_work,
    // Before any trip to the chests: none is made from a pillar, and the want
    // of scaffold blocks must outlive a tick spent aloft.
    perch::aloft,
    supply::scaffold_blocks,
    supply::tools,
    // Before any stance is sought: work in sight needs no walk.
    here::from_here,
    stances::rank_by_walk,
    // Before any pillar: work done from the ground builds, scaffolding does not.
    stances::stance_spots,
    stances::nearest_spot,
    climb::pillars,
    // Before the trip for materials: the trip is long, and the corridor is
    // what the work waits on.
    unreached::finish_way_in,
    // Before digging on: work out of reach must not keep the golem from
    // fetching what reachable work waits for.
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

/// Work is left but none of it can be started this tick.
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

/// Everything buildable is built and no scaffolding is left: head home.
fn finish(projects: &mut Projects, job: &mut Job) -> Step {
    // Asked of the world again before any is reported: a survey entry read
    // mid-change (a fence re-shaped as its neighbour went in) is no loss.
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
    let mut told: Vec<String> = Vec::new();
    match lost {
        0 => {}
        1 => told.push("1 block was lost after it was placed".into()),
        n => told.push(format!("{n} blocks were lost after they were placed")),
    }
    match job.crew.scaffolding.left_standing {
        0 => {}
        1 => told.push("1 scaffold was out of reach and stands".into()),
        n => told.push(format!("{n} scaffolds were out of reach and stand")),
    }
    projects.update(job.id, |p| p.wind_down(told.join("; ")));
    job.crew.note.clear();
    Step::Plan
}

pub(super) fn walk_to(ctx: &Ctx, to: [i32; 3], then: Then) -> Step {
    walk_via(ctx, to, to, then)
}

/// Walk the leg to `to` of the way to `goal`, then `then`.
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
