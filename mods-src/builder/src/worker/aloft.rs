//! Moving on while up high: work not seen from a pillar is walked to along the
//! pillar's course or reached from a scaffold walkway laid from here, before
//! the golem digs down to walk round and climb again.

use crate::host::prelude::*;

use super::bridge::{self, Bridge};
use super::plan::{self, walk_to};
use super::tuning::{
    price::{LEVEL_TICKS, MOVE_TICKS, WALKWAY_TICKS},
    reach::{WALKWAY_CHAIN, WALKWAY_LENGTH},
    waits::SEALED_WAIT,
    window::ALOFT_CANDIDATES,
};
use super::{course, route, Body, Ctx, Pillar, Step, Task, Then};
use crate::geometry::{feet_of, offset, reaches};
use crate::jobs::Job;

enum Way {
    Walk([i32; 3]),
    Walkway(Vec<[i32; 3]>),
}

/// What moving on from where the golem stands up high is weighed against.
struct Outlook {
    /// The pillar's top, the way back down.
    top: [i32; 3],
    /// What a climb down, round and up again costs.
    climb: i32,
    /// The walkway standing out from the pillar, if one does.
    walkway: Option<Bridge>,
    /// How many cells of walkway stand.
    chain: usize,
    /// Whether a walkway may be laid on from where the golem stands.
    lays_from_here: bool,
    /// The stances walked to, and what walking to each (and back) costs in
    /// moves.
    moves: Vec<([i32; 3], i32)>,
}

/// The step on toward the nearest open work reached from up here, or `None`
/// when nothing up here reaches any and the golem goes down.
pub fn move_on(
    ctx: &mut Ctx,
    job: &mut Job,
    body: &Body,
    project: &crate::project::Project,
    perch: Pillar,
    candidates: &[(Task, [i32; 3])],
) -> Option<Step> {
    forget_failed_stances(job, body);
    let Some(outlook) = outlook(ctx, job, body, perch) else {
        return Some(Step::Plan);
    };
    let open: Vec<Task> = candidates
        .iter()
        .map(|(task, _)| *task)
        .filter(|task| matches!(task, Task::Unit(i) if plan::places(job, *task) && !job.crew.deferrals.tried.contains(i)))
        .take(ALOFT_CANDIDATES)
        .collect();
    for task in open {
        let cells = plan::task_cells(job, task);
        let walk = cheapest_walk(job, body, &outlook, task, &cells);
        let Some(way) = way_to(ctx, job, body, &outlook, task, &cells, walk) else {
            continue;
        };
        let stance = match &way {
            Way::Walk(s) => *s,
            Way::Walkway(path) => *path.last()?,
        };
        if plan::waits_there(ctx, job, body, task, stance) {
            continue;
        }
        match plan::cutting(ctx, job, project, task, &cells) {
            Some(false) => {}
            Some(true) => {
                plan::defer_task(ctx, job, task, SEALED_WAIT);
                continue;
            }
            None => return Some(Step::Plan),
        }
        // Laid from there, it must not cut the way back to the top (a walkway
        // not laid yet is asked from its end once it stands).
        if matches!(way, Way::Walk(_)) && stance != outlook.top && plan::places(job, task) {
            let walls = plan::as_walls(ctx, job, &cells);
            match route::probe(ctx, stance, outlook.top, walls) {
                Some(Route::Open) => {}
                Some(_) => {
                    job.crew.deferrals.strike(task, stance);
                    continue;
                }
                None => return Some(Step::Plan),
            }
        }
        return Some(set_out(ctx, job, body, outlook, task, way));
    }
    None
}

/// Walked to for work that was not done there after all, or out along a
/// walkway laid for work not done from its end: never that stance for it
/// again.
fn forget_failed_stances(job: &mut Job, body: &Body) {
    if let Some((task, at)) = job.crew.aloft.aimed.take() {
        if at == body.cell {
            job.crew.deferrals.strike(task, at);
        }
    }
    let Some(walkway) = &job.crew.aloft.bridge else {
        return;
    };
    if walkway.path.last() == Some(&body.cell) {
        let task = walkway.task;
        job.crew.deferrals.strike(task, body.cell);
        if let Task::Unit(i) = task {
            job.crew.deferrals.tried.insert(i);
        }
    }
}

/// Where the golem can move on to from here, and what each costs. `None` =
/// no route budget this tick.
fn outlook(ctx: &mut Ctx, job: &Job, body: &Body, perch: Pillar) -> Option<Outlook> {
    let top = perch.top_cell();
    let walkway = job.crew.aloft.bridge.clone();
    // Stances walked to: with a walkway standing, only its own cells (it
    // comes down on the way back to where it starts, before the golem walks
    // anywhere else); otherwise the pillar's whole course.
    let stances: Vec<[i32; 3]> = match &walkway {
        Some(w) => w.path.clone(),
        None => course::around(ctx, top)?.unwrap_or_default(),
    };
    let region = route::region(ctx, body.cell, false, &[])?;
    let moves: Vec<([i32; 3], i32)> = stances
        .into_iter()
        .filter(|s| *s != body.cell)
        .filter_map(|s| {
            let moves = region?.moves(s)?;
            Some((s, moves as i32))
        })
        .collect();
    // Every step out along the course away from the top is walked back
    // before the golem climbs down.
    let moves = if walkway.is_some() {
        moves
    } else {
        let back = route::region(ctx, top, true, &[])?;
        let from_here = back.and_then(|b| b.moves(body.cell)).unwrap_or(0) as i32;
        moves
            .into_iter()
            .map(|(s, out)| {
                let away = back
                    .and_then(|b| b.moves(s))
                    .map_or(out, |m| m as i32 - from_here);
                (s, out + away.max(0))
            })
            .collect()
    };
    // A walkway goes on from where the golem stands: the end of the one
    // standing, or anywhere on the course.
    let chain = walkway.as_ref().map_or(0, |w| w.path.len());
    let lays_from_here = walkway
        .as_ref()
        .is_none_or(|w| w.path.last() == Some(&body.cell))
        && chain < WALKWAY_CHAIN;
    Some(Outlook {
        top,
        climb: (perch.top - perch.base) * LEVEL_TICKS,
        walkway,
        chain,
        lays_from_here,
        moves,
    })
}

/// The cheapest stance walked to that works `task` (its `cells`), and what
/// the walk costs in ticks.
fn cheapest_walk(
    job: &Job,
    body: &Body,
    outlook: &Outlook,
    task: Task,
    cells: &[[i32; 3]],
) -> Option<([i32; 3], i32)> {
    let near: Vec<([i32; 3], i32)> = outlook
        .moves
        .iter()
        .copied()
        .filter(|(s, _)| {
            reaches(feet_of(*s), cells)
                && plan::clear_of_body(*s, cells)
                && !job.crew.deferrals.blind(task, *s)
                && !job.design.filled.contains(s)
                && !job.design.filled.contains(&offset(*s, [0, 1, 0]))
        })
        .collect();
    if near.is_empty() {
        return None;
    }
    let sees = super::sight::sees(
        body,
        &near.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
        cells,
        &super::sight::work(job, task),
    );
    near.into_iter()
        .zip(sees)
        .filter(|(_, refusal)| refusal.is_none())
        .map(|((s, m), _)| (s, m * MOVE_TICKS))
        .min_by_key(|(_, cost)| *cost)
}

/// The longest walkway worth laying for work walked to at `walk` (its stance
/// and cost), if at all: one only where it beats the walk and a new pillar —
/// a climb back up here and down again for the work, more for a taller one.
fn longest_walkway(outlook: &Outlook, unreachable: bool, walk: Option<([i32; 3], i32)>) -> i32 {
    let mut longest = if outlook.lays_from_here {
        (WALKWAY_CHAIN - outlook.chain).min(WALKWAY_LENGTH as usize) as i32
    } else {
        0
    };
    if !unreachable {
        longest = longest.min(outlook.climb / WALKWAY_TICKS);
    }
    if let Some((_, cost)) = walk {
        longest = longest.min((cost - 1) / WALKWAY_TICKS);
    }
    longest
}

/// How to get within reach of `task`: a walkway laid out to it, or the walk.
fn way_to(
    ctx: &mut Ctx,
    job: &Job,
    body: &Body,
    outlook: &Outlook,
    task: Task,
    cells: &[[i32; 3]],
    walk: Option<([i32; 3], i32)>,
) -> Option<Way> {
    let unreachable = matches!(task, Task::Unit(i) if job.crew.access.unreachable.contains(&i));
    // Past what a climb down, round and up again costs, the ground way is
    // as good, and keeps the golem off long walks along wall tops.
    let walk = if !unreachable && outlook.walkway.is_none() {
        walk.filter(|(_, cost)| *cost <= outlook.climb)
    } else {
        walk
    };
    let longest = longest_walkway(outlook, unreachable, walk);
    let walkway = (longest > 0)
        .then(|| bridge::plan(ctx, job, body, task, cells, longest, outlook.walkway.as_ref()))
        .flatten();
    match walkway {
        Some(path) => Some(Way::Walkway(path)),
        None => walk.map(|(s, _)| Way::Walk(s)),
    }
}

/// Set out along `way` for `task`.
fn set_out(
    ctx: &mut Ctx,
    job: &mut Job,
    body: &Body,
    outlook: Outlook,
    task: Task,
    way: Way,
) -> Step {
    match way {
        Way::Walk(s) => {
            trace!(
                "TRACE aloft: walking from {:?} to {s:?} for {task:?}",
                body.cell
            );
            job.crew.aloft.aimed = Some((task, s));
            walk_to(ctx, s, Then::Course(task))
        }
        Way::Walkway(path) => {
            trace!(
                "TRACE aloft: walkway from {:?} along {path:?} for {task:?} (chain {})",
                body.cell,
                outlook.chain
            );
            job.crew.aloft.bridge = Some(match outlook.walkway {
                Some(mut w) => {
                    w.path.extend(path);
                    w.task = task;
                    w
                }
                None => Bridge {
                    top: body.cell,
                    path,
                    task,
                },
            });
            Step::Bridge {
                since: ctx.now,
                asked: 0,
            }
        }
    }
}

#[cfg(test)]
mod tests;
