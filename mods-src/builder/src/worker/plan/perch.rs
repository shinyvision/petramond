//! Planning from the top of a pillar, a roof course or a walkway.

use mod_sdk::*;

use super::here::{behind, clear_of_body, sees_from_here};
use super::sealing::{as_walls, cutting};
use super::verdict::{settle_verdict, verdict_name, viability, Viable};
use super::{defer_task, places, task_cells, trace, walk_to, Flow, Mode, Round};
use crate::geometry::{feet_of, offset, reaches};
use crate::jobs::Job;
use crate::project::Projects;
use crate::survey::Known;
use crate::worker::body::stands_at;
use crate::worker::tuning::body::PERCH_OFF_CENTRE;
use crate::worker::tuning::patience::RAISES;
use crate::worker::tuning::reach::{MAX_HEIGHT, RAISE_ASKS, RAISE_LEVELS};
use crate::worker::tuning::waits::{IDLE_CLIMB, SEALED_WAIT};
use crate::worker::upkeep::open_block;
use crate::worker::waiting::{Probe, Waiting};
use crate::worker::{
    aloft, course, pillar, route, scaffold, sight, Body, Ctx, Step, Task, Then, TRACE,
};

/// Whether the golem, out on a course at `body.cell`, still walks back to the
/// pillar's `top` once `cells` stand. `None` = no route budget this tick.
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

/// The first open placement seen from where the golem stands out on a roof
/// course that it has not just tried from here.
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
                // No budget is no answer: ask again rather than walk away.
                None => return Some(Step::Plan),
                Some(route) => {
                    trace!("TRACE try here {task:?}: way back to {top:?} {route:?}");
                    job.crew.deferrals.strike(task, body.cell);
                    continue;
                }
            }
        }
        // Once until something lands: a landed block gives its neighbours a
        // face and every one gets another try.
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
        job.crew.deferrals.deferred.remove(&task);
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
    let mut higher = false;
    let top = perch.top_cell();
    // The perch was chosen for what its centre sees; judged from the edge the
    // jump left the golem on, a wall's corner hides it.
    let [dx, dz] = body.off_centre();
    if body.cell == top
        && dx.abs().max(dz.abs()) > PERCH_OFF_CENTRE
        && ctx.now < job.crew.aloft.settle_until
    {
        return Flow::Go(Step::Plan);
    }
    // Likewise at a walkway's end: a reach judged from its rim fell short
    // and the walkway was taken down and laid again, over and over.
    let at_walkway_end = job
        .crew
        .aloft
        .bridge
        .as_ref()
        .is_some_and(|walkway| walkway.path.last() == Some(&body.cell));
    if at_walkway_end && dx.abs().max(dz.abs()) > PERCH_OFF_CENTRE {
        return Flow::Go(Step::Plan);
    }
    // And at a stance walked to along the course.
    if job.crew.aloft.aimed.is_some_and(|(_, at)| at == body.cell)
        && dx.abs().max(dz.abs()) > PERCH_OFF_CENTRE
        && ctx.now < job.crew.aloft.settle_until
    {
        return Flow::Go(Step::Plan);
    }
    // What the climb or the walkway was planned for is done first, as
    // planned: it was checked then, and is only passed over if the world no
    // longer lets it be seen.
    let committed = match &job.crew.aloft.bridge {
        Some(walkway) if body.cell == *walkway.path.last().unwrap_or(&top) => Some(walkway.task),
        Some(_) => None,
        // From the top or wherever the climb's course led: the onward
        // stance was chosen for it.
        None => job.crew.aloft.climbed_for.map(|(task, _)| task),
    };
    if let (true, Some(task)) = (TRACE, committed) {
        trace::committed(ctx, job, body, task);
    }
    // Off a walkway it still counts as laid (knocked off it, stepped down
    // onto a roof beside it): it comes down from where the golem stands.
    if job
        .crew
        .aloft
        .bridge
        .as_ref()
        .is_some_and(|w| w.top != body.cell && !w.path.contains(&body.cell))
    {
        return Flow::Go(Step::Unbridge { since: ctx.now });
    }
    // Going home, what the climb was for is left unlaid.
    let committed = committed.filter(|task| mode == Mode::Building || !places(job, *task));
    if let Some(task) = committed {
        let open = match (task, job.survey.as_ref()) {
            (Task::Unit(i), Some(s)) => s.known[i].open() && !job.crew.built.contains(&i),
            // Only while what it opens a way to still waits: re-laid once that
            // stands, a climb's reopen would be dug out again forever.
            (Task::Reopen(o), Some(s)) => {
                matches!(s.known[o], Known::Satisfied)
                    && job.crew.access.reopen.values().flatten().any(|v| *v == o)
            }
            (Task::Trim(cell), _) => job.crew.access.trims.contains(&cell),
            // A stray scaffold up high is work a pillar is climbed for too.
            (Task::Scaffold(cell), _) => {
                project.scaffolds.contains(&cell)
                    && !perch.holds(cell)
                    && scaffold::stands(ctx.content, cell) == Some(true)
            }
            _ => false,
        };
        let cells = task_cells(job, task);
        // Walked out to for it and not workable after all: never this stance
        // for it again, or the same climb is planned and left forever.
        if open && body.cell != top && !sees_from_here(job, task, body, &cells) {
            job.crew.deferrals.strike(task, body.cell);
        }
        if open
            && sees_from_here(job, task, body, &cells)
            && !job.crew.deferrals.tried.contains(&task.unit())
            && viability(ctx, job, body, task, body.pos) == Viable::Now
        {
            // Planned or not, it must not shut the way back to the top, nor
            // wall ground off.
            if body.cell != top && places(job, task) {
                match way_back(ctx, job, body, top, &cells) {
                    Some(Route::Open) => {}
                    None => return Flow::Go(Step::Plan),
                    Some(_) => {
                        // Laid from here it strands the golem out here: never
                        // this stance for it again, or the climb repeats.
                        job.crew.deferrals.strike(task, body.cell);
                        job.crew.deferrals.tried.insert(task.unit());
                        defer_task(ctx, job, task, SEALED_WAIT);
                        return Flow::Go(walk_to(ctx, top, Then::Regroup));
                    }
                }
            }
            match cutting(ctx, job, project, task, &cells) {
                Some(false) => {}
                None => return Flow::Go(Step::Plan),
                Some(true) => {
                    job.crew.deferrals.tried.insert(task.unit());
                    defer_task(ctx, job, task, SEALED_WAIT);
                    return Flow::Go(Step::Plan);
                }
            }
            job.crew.deferrals.tried.insert(task.unit());
            job.crew.deferrals.deferred.remove(&task);
            return Flow::Go(Step::Centre {
                task,
                since: ctx.now,
            });
        }
    }
    // From up here the golem's ground is never stranded (it walked to the
    // pillar's foot first), but ground must not be walled off nor the way back
    // to the top cut; that is asked where it stands. Off the pillar's course
    // altogether (fallen, knocked down), the top is no longer the way anywhere.
    if job.crew.aloft.bridge.is_none() && body.cell != top {
        let Some(around) = course::around(ctx, top) else {
            return Flow::Busy(Waiting::Probe(Probe::CourseFlood));
        };
        if let Some(footholds) = around {
            if !footholds.contains(&body.cell) {
                trace!("TRACE off the course of {top:?} at {:?}", body.cell);
                job.crew.aloft.dismount_lost();
                job.crew.deferrals.tried.clear();
                job.crew.trail.broken();
                return Flow::Go(Step::Plan);
            }
        }
    }
    for (task, _) in candidates {
        let cells = task_cells(job, *task);
        if !job.crew.deferrals.blind(*task, body.cell) && sees_from_here(job, *task, body, &cells) {
            let verdict = viability(ctx, job, body, *task, body.pos);
            if verdict != Viable::Now {
                settle_verdict(ctx, job, *task, verdict);
                continue;
            }
            // Out on a roof course, the way back to the top must survive it.
            if body.cell != top {
                match way_back(ctx, job, body, top, &cells) {
                    Some(Route::Open) => {}
                    Some(_) => {
                        defer_task(ctx, job, *task, SEALED_WAIT);
                        continue;
                    }
                    None => return Flow::Go(Step::Plan),
                }
            }
            match cutting(ctx, job, project, *task, &cells) {
                Some(false) => {
                    let task = behind(ctx, job, project, mode, body, *task).unwrap_or(*task);
                    return Flow::Go(Step::Centre {
                        task,
                        since: ctx.now,
                    });
                }
                Some(true) => defer_task(ctx, job, *task, SEALED_WAIT),
                None => return Flow::Go(Step::Plan),
            }
        } else if body.cell == top && perch.extends_to(ctx, body, &cells, &job.design.governed) {
            higher = true;
        }
    }
    trace::perch_top(job, body, top, candidates);
    // Up a pillar or out on a roof course: every block it sees gets a try
    // before it leaves, waits or no. A climb is dear, and a ridge whose blocks
    // wait on each other for a face only starts when one is tried.
    // (A job going home lays nothing more.)
    if let Some(step) = try_here(ctx, job, body, project, mode, perch) {
        return Flow::Go(step);
    }
    // On the way down the pillar is never raised again, but a level still
    // bridges out to work it reaches and cannot see: the pillar underfoot is
    // paid for, and the other way to that block is digging this one down and
    // raising another beside it. The walkway's price against what is left of
    // the climb keeps it from bridging near the ground.
    if job.crew.aloft.descending.is_some() {
        if let Some(step) = aloft::move_on(ctx, job, body, project, perch, candidates) {
            return Flow::Go(step);
        }
        if body.cell != top {
            if job.crew.aloft.bridge.is_some() {
                return Flow::Go(Step::Unbridge { since: ctx.now });
            }
            return Flow::Go(walk_to(ctx, top, Then::Regroup));
        }
        return Flow::Go(Step::Descend { since: ctx.now });
    }
    if body.cell == top && job.crew.aloft.bridge.is_none() {
        if let Some(stance) = perch.onward {
            // The course it was planned along may have changed under it since
            // (overgrowth it stood on cut away): walking on at a hole is a fall.
            let standing = stands_at(stance);
            let walks = standing
                && match route::probe(ctx, body.cell, stance, Vec::new()) {
                    Some(route) => route == Route::Open,
                    None => {
                        return Flow::Busy(Waiting::Probe(Probe::Onward));
                    }
                };
            job.crew.aloft.perch = Some(pillar::Pillar {
                onward: None,
                ..perch
            });
            if walks {
                return Flow::Go(walk_to(ctx, stance, Then::Regroup));
            }
            return Flow::Go(Step::Plan);
        }
        let levels = if higher {
            Some(1)
        } else {
            match raise_for(ctx, job, body, perch, candidates) {
                Ok(levels) => levels,
                Err(()) => return Flow::Go(Step::Plan),
            }
        };
        if let Some(levels) = levels {
            let mut raised = perch;
            raised.top += levels;
            job.crew.aloft.raises += 1;
            job.crew.aloft.dismount_to_raise();
            return Flow::Go(Step::Climb {
                pillar: raised,
                level: None,
                placed: false,
                since: ctx.now,
            });
        }
    }
    // Work up here out of sight: along the course or out on a walkway before
    // down and round and up again.
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
    // A climb that did no work is not planned again: not this perch for this
    // task, however long it waits.
    if let Some((task, 0)) = job.crew.aloft.climbed_for.take() {
        job.crew.deferrals.strike(task, top);
        defer_task(ctx, job, task, IDLE_CLIMB);
    }
    job.crew.deferrals.tried.clear();
    job.crew.aloft.descending = Some(body.cell[1]);
    Flow::Go(Step::Descend { since: ctx.now })
}

/// How many levels higher the golem's pillar would have to be for it to lay
/// open work beside it that it cannot see from here (a roof slope climbs away
/// from a pillar at its eave): a few blocks underfoot cost far less than
/// climbing down and raising another. `Err` = no answer this tick.
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
    // Open air over the head all the way up, none of it the design's.
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
    // The lowest level that lays the most.
    let best = lays.iter().copied().max().unwrap_or(0);
    if best == 0 {
        return Ok(None);
    }
    Ok(lays.iter().position(|n| *n == best).map(|l| l as i32 + 1))
}

/// Up a pillar, the plan is made from there and nowhere else.
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
