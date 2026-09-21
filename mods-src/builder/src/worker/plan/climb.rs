//! Setting out for a pillar's foot to climb it.

use super::sealing::cutting;
use super::unreached::fall_out;
use super::verdict::waits_there;
use super::{defer_task, places, task_cells, walk_via, Flow, Round};
use crate::jobs::Job;
use crate::project::Projects;
use crate::worker::route::{self, Hubs};
use crate::worker::tuning::waits::{FOOT_UNWALKED, SCAFFOLD_WAIT, SEALED_WAIT};
use crate::worker::waiting::{Probe, Waiting};
use crate::worker::{pillar, scaffold, stance, Body, Ctx, Task, Then};

/// Walk to a pillar's foot to climb it (in legs when the way is long); `None`
/// when the task waits instead: the foot cannot be walked to, or the task
/// would be refused at the top anyway (a climb for nothing, then the pillar
/// comes straight down).
#[allow(clippy::too_many_arguments)]
pub(super) fn climb_toward(
    ctx: &mut Ctx,
    job: &mut Job,
    project: &crate::project::Project,
    hubs: Hubs,
    body: &Body,
    task: Task,
    cells: &[[i32; 3]],
    pillar: pillar::Pillar,
) -> Option<Flow> {
    if waits_there(ctx, job, body, task, pillar.top_cell()) {
        return None;
    }
    match cutting(ctx, job, project, task, cells) {
        Some(false) => {}
        Some(true) => {
            defer_task(ctx, job, task, SEALED_WAIT);
            return None;
        }
        None => {
            return Some(Flow::Busy(Waiting::Probe(Probe::ClimbSealing)));
        }
    }
    // A pillar is made of blocks the golem carries: short of its height,
    // the chests are asked first.
    let levels = (pillar.top - pillar.base).max(0) as u32;
    if scaffold::in_hand(ctx, job, &body.slots) < levels {
        job.crew.scaffolding.want = levels;
        job.crew.scaffolding.short = true;
        defer_task(ctx, job, task, SCAFFOLD_WAIT);
        return None;
    }
    let foot = pillar.foot_cell();
    match route::leg(ctx, hubs, body, foot) {
        Some(Some(cell)) => {
            job.crew.aloft.climbed_for = Some((task, 0));
            job.crew.aloft.raises = 0;
            Some(Flow::Go(walk_via(ctx, cell, foot, Then::Climb(pillar))))
        }
        Some(None) => {
            defer_task(ctx, job, task, FOOT_UNWALKED);
            None
        }
        None => Some(Flow::Busy(Waiting::Probe(Probe::Walk))),
    }
}

/// Work no stance on foot reaches gets a pillar, or one whose top walks on
/// to it; work nothing reaches falls out of the plan for a while.
pub(super) fn pillars(
    ctx: &mut Ctx,
    projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    round: &mut Round,
) -> Flow {
    let project = &round.project;
    let candidates = &round.candidates;
    for (task, unseen) in &round.wants_perch {
        let unseen = *unseen;
        if job.crew.deferrals.deferred(*task, ctx.now) {
            continue;
        }
        let cells = task_cells(job, *task);
        let digging = !places(job, *task);
        let trail = job.crew.trail.clone();
        let hubs = Hubs::new(project.home, &trail);
        let open: Vec<[i32; 3]> = candidates.iter().map(|(_, c)| *c).collect();
        match pillar::find(ctx, job, project, body, *task, &cells, &open) {
            stance::Search::Found(pillar) => {
                match climb_toward(ctx, job, project, hubs, body, *task, &cells, pillar) {
                    Some(flow) => return flow,
                    None => continue,
                }
            }
            stance::Search::Busy => {
                return Flow::Busy(Waiting::Probe(Probe::Pillar));
            }
            stance::Search::None | stance::Search::Unseen => {}
        }
        let onward = pillar::find_onward(ctx, job, project, body, *task, &cells);
        match onward {
            stance::Search::Found(pillar) => {
                match climb_toward(ctx, job, project, hubs, body, *task, &cells, pillar) {
                    Some(flow) => return flow,
                    None => continue,
                }
            }
            stance::Search::Busy => {
                return Flow::Busy(Waiting::Probe(Probe::OnwardPillar));
            }
            stance::Search::None | stance::Search::Unseen => {
                let flow = fall_out(
                    ctx, projects, job, body, project, *task, unseen, &cells, digging,
                );
                if let Some(flow) = flow {
                    return flow;
                }
            }
        }
    }
    Flow::Pass
}
