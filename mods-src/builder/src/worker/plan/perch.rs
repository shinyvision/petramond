use std::ops::ControlFlow;

use crate::host::prelude::*;

use super::here::{behind, clear_of_body, sees_from_here};
use super::sealing::{as_walls, cutting};
use super::verdict::{settle_verdict, verdict_name, viability, Viable};
use super::{defer_task, places, task_cells, trace, walk_to, Flow, Mode, Round};
use crate::geometry::{feet_of, offset, reaches};
use crate::project::Projects;
use crate::survey::Known;
use crate::worker::body::stands_at;
use crate::worker::tuning::body::PERCH_OFF_CENTRE;
use crate::worker::tuning::patience::RAISES;
use crate::worker::tuning::reach::{MAX_HEIGHT, RAISE_ASKS, RAISE_LEVELS};
use crate::worker::tuning::waits::{IDLE_CLIMB, SEALED_WAIT};
use crate::worker::upkeep::open_block;
use crate::worker::waiting::{Probe, Waiting};
use crate::worker::Job;
use crate::worker::{
    aloft, course, pillar, route, scaffold, sight, Body, Ctx, Step, Task, Then, TRACE,
};

fn way_back(
    ctx: &mut Ctx,
    job: &Job,
    body: &Body,
    top: [i32; 3],
    cells: &[[i32; 3]],
) -> Option<Route> {
    let walls = as_walls(ctx, job, cells);
    route::probe(ctx, body.cell, top, walls)
}

fn try_here(
    ctx: &mut Ctx,
    job: &mut Job,
    body: &Body,
    project: &crate::project::Project,
    mode: Mode,
    perch: pillar::Pillar,
) -> Option<Step> {
    if mode != Mode::Building {
        return None;
    }
    let top = perch.top_cell();
    let survey = job.survey.as_ref()?;
    let r = crate::geometry::PLAN_REACH.ceil() as i32;
    let mut near = Vec::new();
    for dx in -r..=r {
        for dy in -r..=r + 1 {
            for dz in -r..=r {
                if let Some(i) = job.design.unit_at(offset(body.cell, [dx, dy, dz])) {
                    if matches!(survey.known[i], Known::Place(_))
                        && !job.crew.built.contains(&i)
                        && !job.crew.deferrals.tried.contains(&i)
                    {
                        near.push(i);
                    }
                }
            }
        }
    }
    for i in near {
        let task = Task::Unit(i);
        let cells = task_cells(job, task);
        if !sees_from_here(job, task, body, &cells) {
            continue;
        }
        if body.cell != top {
            match way_back(ctx, job, body, top, &cells) {
                Some(Route::Open) => {}
                None => return Some(Step::Plan),
                Some(route) => {
                    trace!("TRACE try here {task:?}: way back to {top:?} {route:?}");
                    job.crew.deferrals.strike(task, body.cell);
                    continue;
                }
            }
        }
        job.crew.deferrals.tried.insert(i);
        let verdict = viability(ctx, job, body, task, body.pos);
        if verdict != Viable::Now {
            trace!("TRACE try here {task:?}: {}", verdict_name(verdict));
            continue;
        }
        match cutting(ctx, job, project, task, &cells) {
            Some(false) => {}
            None => {
                job.crew.deferrals.tried.remove(&i);
                return Some(Step::Plan);
            }
            Some(true) => {
                trace!("TRACE try here {task:?}: seals");
                continue;
            }
        }
        job.crew.deferrals.deferred.lift(&task);
        return Some(Step::Centre {
            task,
            since: ctx.now,
        });
    }
    None
}

pub(super) fn from_perch(
    ctx: &mut Ctx,
    job: &mut Job,
    body: &Body,
    project: &crate::project::Project,
    mode: Mode,
    perch: pillar::Pillar,
    candidates: &[(Task, [i32; 3])],
) -> Flow {
    let top = perch.top_cell();
    if unsettled(ctx, job, body, top) {
        return Flow::Go(Step::Plan);
    }
    if let Some(flow) = work_committed(ctx, job, body, project, mode, perch) {
        return flow;
    }
    if let Some(flow) = off_the_course(ctx, job, body, top) {
        return flow;
    }
    let higher = match work_in_sight(ctx, job, body, project, mode, perch, candidates) {
        ControlFlow::Break(flow) => return flow,
        ControlFlow::Continue(higher) => higher,
    };
    trace::perch_top(job, body, top, candidates);
    if let Some(step) = try_here(ctx, job, body, project, mode, perch) {
        return Flow::Go(step);
    }
    if job.crew.aloft.descending.is_some() {
        return descending(ctx, job, body, project, perch, candidates);
    }
    if body.cell == top && job.crew.aloft.bridge.is_none() {
        if let Some(flow) = onward_or_higher(ctx, job, body, perch, candidates, higher) {
            return flow;
        }
    }
    leave(ctx, job, body, project, perch, candidates)
}

fn unsettled(ctx: &Ctx, job: &Job, body: &Body, top: [i32; 3]) -> bool {
    let [dx, dz] = body.off_centre();
    if dx.abs().max(dz.abs()) <= PERCH_OFF_CENTRE {
        return false;
    }
    let settling = ctx.now < job.crew.aloft.settle_until;
    if body.cell == top && settling {
        return true;
    }
    let at_walkway_end = job
        .crew
        .aloft
        .bridge
        .as_ref()
        .is_some_and(|walkway| walkway.path.last() == Some(&body.cell));
    if at_walkway_end {
        return true;
    }
    settling && job.crew.aloft.aimed.is_some_and(|(_, at)| at == body.cell)
}

fn work_committed(
    ctx: &mut Ctx,
    job: &mut Job,
    body: &Body,
    project: &crate::project::Project,
    mode: Mode,
    perch: pillar::Pillar,
) -> Option<Flow> {
    let top = perch.top_cell();
    let committed = match &job.crew.aloft.bridge {
        Some(walkway) if body.cell == *walkway.path.last().unwrap_or(&top) => Some(walkway.task),
        Some(_) => None,
        None => job.crew.aloft.climbed_for.map(|(task, _)| task),
    };
    if let (true, Some(task)) = (TRACE, committed) {
        trace::committed(ctx, job, body, task);
    }
    if job
        .crew
        .aloft
        .bridge
        .as_ref()
        .is_some_and(|w| w.top != body.cell && !w.path.contains(&body.cell))
    {
        return Some(Flow::Go(Step::Unbridge { since: ctx.now }));
    }
    let task = committed.filter(|task| mode == Mode::Building || !places(job, *task))?;
    let open = still_open(ctx, job, project, perch, task);
    let cells = task_cells(job, task);
    if open && body.cell != top && !sees_from_here(job, task, body, &cells) {
        job.crew.deferrals.strike(task, body.cell);
    }
    if !open
        || !sees_from_here(job, task, body, &cells)
        || job.crew.deferrals.tried.contains(&task.unit())
        || viability(ctx, job, body, task, body.pos) != Viable::Now
    {
        return None;
    }
    if body.cell != top && places(job, task) {
        match way_back(ctx, job, body, top, &cells) {
            Some(Route::Open) => {}
            None => return Some(Flow::Go(Step::Plan)),
            Some(_) => {
                job.crew.deferrals.strike(task, body.cell);
                job.crew.deferrals.tried.insert(task.unit());
                defer_task(ctx, job, task, SEALED_WAIT);
                return Some(Flow::Go(walk_to(ctx, top, Then::Regroup)));
            }
        }
    }
    match cutting(ctx, job, project, task, &cells) {
        Some(false) => {}
        None => return Some(Flow::Go(Step::Plan)),
        Some(true) => {
            job.crew.deferrals.tried.insert(task.unit());
            defer_task(ctx, job, task, SEALED_WAIT);
            return Some(Flow::Go(Step::Plan));
        }
    }
    job.crew.deferrals.tried.insert(task.unit());
    job.crew.deferrals.deferred.lift(&task);
    Some(Flow::Go(Step::Centre {
        task,
        since: ctx.now,
    }))
}

fn still_open(
    ctx: &Ctx,
    job: &Job,
    project: &crate::project::Project,
    perch: pillar::Pillar,
    task: Task,
) -> bool {
    match (task, job.survey.as_ref()) {
        (Task::Unit(i), Some(s)) => s.known[i].open() && !job.crew.built.contains(&i),
        (Task::Reopen(o), Some(s)) => {
            matches!(s.known[o], Known::Satisfied)
                && job.crew.access.reopen.values().flatten().any(|v| *v == o)
        }
        (Task::Trim(cell), _) => job.crew.access.trims.contains(&cell),
        (Task::Scaffold(cell), _) => {
            project.scaffolds.contains(&cell)
                && !perch.holds(cell)
                && scaffold::stands(ctx.content, cell) == Some(true)
        }
        _ => false,
    }
}

fn off_the_course(ctx: &mut Ctx, job: &mut Job, body: &Body, top: [i32; 3]) -> Option<Flow> {
    if job.crew.aloft.bridge.is_some() || body.cell == top {
        return None;
    }
    let Some(around) = course::around(ctx, top) else {
        return Some(Flow::Busy(Waiting::Probe(Probe::CourseFlood)));
    };
    if around.is_some_and(|footholds| !footholds.contains(&body.cell)) {
        trace!("TRACE off the course of {top:?} at {:?}", body.cell);
        job.crew.aloft.dismount_lost();
        job.crew.deferrals.tried.clear();
        job.crew.trail.broken();
        return Some(Flow::Go(Step::Plan));
    }
    None
}

fn work_in_sight(
    ctx: &mut Ctx,
    job: &mut Job,
    body: &Body,
    project: &crate::project::Project,
    mode: Mode,
    perch: pillar::Pillar,
    candidates: &[(Task, [i32; 3])],
) -> ControlFlow<Flow, bool> {
    let top = perch.top_cell();
    let mut higher = false;
    for (task, _) in candidates {
        let cells = task_cells(job, *task);
        if !job.crew.deferrals.blind(*task, body.cell) && sees_from_here(job, *task, body, &cells) {
            let verdict = viability(ctx, job, body, *task, body.pos);
            if verdict != Viable::Now {
                settle_verdict(ctx, job, *task, verdict);
                continue;
            }
            if body.cell != top {
                match way_back(ctx, job, body, top, &cells) {
                    Some(Route::Open) => {}
                    Some(_) => {
                        defer_task(ctx, job, *task, SEALED_WAIT);
                        continue;
                    }
                    None => return ControlFlow::Break(Flow::Go(Step::Plan)),
                }
            }
            match cutting(ctx, job, project, *task, &cells) {
                Some(false) => {
                    let task = behind(ctx, job, project, mode, body, *task).unwrap_or(*task);
                    return ControlFlow::Break(Flow::Go(Step::Centre {
                        task,
                        since: ctx.now,
                    }));
                }
                Some(true) => defer_task(ctx, job, *task, SEALED_WAIT),
                None => return ControlFlow::Break(Flow::Go(Step::Plan)),
            }
        } else if body.cell == top && perch.extends_to(ctx, body, &cells, &job.design.governed) {
            higher = true;
        }
    }
    ControlFlow::Continue(higher)
}

fn descending(
    ctx: &mut Ctx,
    job: &mut Job,
    body: &Body,
    project: &crate::project::Project,
    perch: pillar::Pillar,
    candidates: &[(Task, [i32; 3])],
) -> Flow {
    let top = perch.top_cell();
    if let Some(step) = aloft::move_on(ctx, job, body, project, perch, candidates) {
        return Flow::Go(step);
    }
    if body.cell != top {
        if job.crew.aloft.bridge.is_some() {
            return Flow::Go(Step::Unbridge { since: ctx.now });
        }
        return Flow::Go(walk_to(ctx, top, Then::Regroup));
    }
    Flow::Go(Step::Descend { since: ctx.now })
}

fn onward_or_higher(
    ctx: &mut Ctx,
    job: &mut Job,
    body: &Body,
    perch: pillar::Pillar,
    candidates: &[(Task, [i32; 3])],
    higher: bool,
) -> Option<Flow> {
    if let Some(stance) = perch.onward {
        let standing = stands_at(stance);
        let walks = standing
            && match route::probe(ctx, body.cell, stance, Vec::new()) {
                Some(route) => route == Route::Open,
                None => return Some(Flow::Busy(Waiting::Probe(Probe::Onward))),
            };
        job.crew.aloft.perch = Some(pillar::Pillar {
            onward: None,
            ..perch
        });
        if walks {
            return Some(Flow::Go(walk_to(ctx, stance, Then::Regroup)));
        }
        return Some(Flow::Go(Step::Plan));
    }
    let levels = if higher {
        1
    } else {
        match raise_for(ctx, job, body, perch, candidates) {
            Ok(Some(levels)) => levels,
            Ok(None) => return None,
            Err(()) => return Some(Flow::Go(Step::Plan)),
        }
    };
    let mut raised = perch;
    raised.top += levels;
    job.crew.aloft.raises += 1;
    job.crew.aloft.dismount_to_raise();
    Some(Flow::Go(Step::Climb {
        pillar: raised,
        level: None,
        placed: false,
        since: ctx.now,
    }))
}

fn leave(
    ctx: &mut Ctx,
    job: &mut Job,
    body: &Body,
    project: &crate::project::Project,
    perch: pillar::Pillar,
    candidates: &[(Task, [i32; 3])],
) -> Flow {
    let top = perch.top_cell();
    if let Some(step) = aloft::move_on(ctx, job, body, project, perch, candidates) {
        return Flow::Go(step);
    }
    if body.cell != top {
        if job.crew.aloft.bridge.is_some() {
            return Flow::Go(Step::Unbridge { since: ctx.now });
        }
        job.crew.deferrals.tried.clear();
        return Flow::Go(walk_to(ctx, top, Then::Regroup));
    }
    if let Some((task, 0)) = job.crew.aloft.climbed_for.take() {
        job.crew.deferrals.strike(task, top);
        defer_task(ctx, job, task, IDLE_CLIMB);
    }
    job.crew.deferrals.tried.clear();
    job.crew.aloft.descending = Some(body.cell[1]);
    Flow::Go(Step::Descend { since: ctx.now })
}

fn raise_for(
    ctx: &mut Ctx,
    job: &mut Job,
    body: &Body,
    perch: pillar::Pillar,
    candidates: &[(Task, [i32; 3])],
) -> Result<Option<i32>, ()> {
    if job.crew.aloft.raises >= RAISES {
        return Ok(None);
    }
    let room = (MAX_HEIGHT - (perch.top - perch.base)).min(RAISE_LEVELS);
    if room <= 0 {
        return Ok(None);
    }
    let over: Vec<[i32; 3]> = (2..2 + room)
        .map(|dy| [perch.column[0], perch.top + dy, perch.column[1]])
        .collect();
    let mut clear = 0;
    for (cell, block) in over.iter().zip(get_blocks(over.clone())) {
        let Some(block) = block else {
            return Err(());
        };
        if !open_block(ctx, block) || job.design.governed.contains(cell) {
            break;
        }
        clear += 1;
    }
    let mut lays = vec![0; clear as usize];
    let open: Vec<Task> = candidates
        .iter()
        .map(|(task, _)| *task)
        .filter(|task| matches!(task, Task::Unit(i) if places(job, *task) && !job.crew.deferrals.tried.contains(i)))
        .take(RAISE_ASKS)
        .collect();
    for task in open {
        let cells = task_cells(job, task);
        let feet: Vec<[f64; 3]> = (1..=clear)
            .map(|l| [body.pos[0], body.pos[1] + f64::from(l), body.pos[2]])
            .collect();
        let usable: Vec<bool> = (1..=clear)
            .map(|l| {
                let stand = offset(body.cell, [0, l, 0]);
                reaches(feet_of(stand), &cells)
                    && clear_of_body(stand, &cells)
                    && !job.crew.deferrals.blind(task, stand)
            })
            .collect();
        if !usable.iter().any(|ok| *ok) {
            continue;
        }
        let sees = sight::sees_from(body, feet, &cells, &sight::work(job, task));
        for ((count, ok), refusal) in lays.iter_mut().zip(usable).zip(sees) {
            *count += i32::from(ok && refusal.is_none());
        }
    }
    let best = lays.iter().copied().max().unwrap_or(0);
    if best == 0 {
        return Ok(None);
    }
    Ok(lays.iter().position(|n| *n == best).map(|l| l as i32 + 1))
}

pub(super) fn aloft(
    ctx: &mut Ctx,
    _projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    round: &mut Round,
) -> Flow {
    if let Some(perch) = job.crew.aloft.perch {
        return from_perch(
            ctx,
            job,
            body,
            &round.project,
            round.mode,
            perch,
            &round.candidates,
        );
    }
    Flow::Pass
}
