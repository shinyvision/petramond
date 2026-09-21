//! Moving on while up high: work not seen from a pillar is walked to along the
//! pillar's course or reached from a scaffold walkway laid from here, before
//! the golem digs down to walk round and climb again.

use mod_sdk::*;

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
    let top = perch.top_cell();
    // Walked to for work that was not done there after all: never that
    // stance for it again.
    if let Some((task, at)) = job.crew.aloft.aimed.take() {
        if at == body.cell {
            job.crew.deferrals.strike(task, at);
        }
    }
    let walkway = job.crew.aloft.bridge.clone();
    if let Some(walkway) = &walkway {
        if walkway.path.last() == Some(&body.cell) {
            // Laid for work not done from its end: not laid for it again.
            job.crew.deferrals.strike(walkway.task, body.cell);
            if let Task::Unit(i) = walkway.task {
                job.crew.deferrals.tried.insert(i);
            }
        }
    }
    // Stances walked to: with a walkway standing, only its own cells (it
    // comes down on the way back to where it starts, before the golem walks
    // anywhere else); otherwise the pillar's whole course.
    let stances: Vec<[i32; 3]> = match &walkway {
        Some(w) => w.path.clone(),
        None => match course::around(ctx, top) {
            None => return Some(Step::Plan),
            Some(None) => Vec::new(),
            Some(Some(course)) => course,
        },
    };
    let moves: Vec<([i32; 3], i32)> = {
        let region = match route::region(ctx, body.cell, false, &[]) {
            None => return Some(Step::Plan),
            Some(region) => region,
        };
        stances
            .into_iter()
            .filter(|s| *s != body.cell)
            .filter_map(|s| {
                let moves = region?.moves(s)?;
                Some((s, moves as i32))
            })
            .collect()
    };
    // Every step out along the course away from the top is walked back
    // before the golem climbs down.
    let moves: Vec<([i32; 3], i32)> = if walkway.is_some() {
        moves
    } else {
        let back = match route::region(ctx, top, true, &[]) {
            None => return Some(Step::Plan),
            Some(back) => back,
        };
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
    let open: Vec<Task> = candidates
        .iter()
        .map(|(task, _)| *task)
        .filter(|task| matches!(task, Task::Unit(i) if plan::places(job, *task) && !job.crew.deferrals.tried.contains(i)))
        .take(ALOFT_CANDIDATES)
        .collect();
    for task in open {
        let cells = plan::task_cells(job, task);
        let near: Vec<([i32; 3], i32)> = moves
            .iter()
            .copied()
            .filter(|(s, _)| {
                reaches(feet_of(*s), &cells)
                    && plan::clear_of_body(*s, &cells)
                    && !job.crew.deferrals.blind(task, *s)
                    && !job.design.filled.contains(s)
                    && !job.design.filled.contains(&offset(*s, [0, 1, 0]))
            })
            .collect();
        let mut walk: Option<([i32; 3], i32)> = None;
        if !near.is_empty() {
            let sees = super::sight::sees(
                body,
                &near.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
                &cells,
                &super::sight::work(job, task),
            );
            walk = near
                .into_iter()
                .zip(sees)
                .filter(|(_, refusal)| refusal.is_none())
                .map(|((s, m), _)| (s, m * MOVE_TICKS))
                .min_by_key(|(_, cost)| *cost);
        }
        let unreachable = matches!(task, Task::Unit(i) if job.crew.access.unreachable.contains(&i));
        // Past what a climb down, round and up again costs, the ground way is
        // as good, and keeps the golem off long walks along wall tops.
        let climb = (perch.top - perch.base) * LEVEL_TICKS;
        if !unreachable && walkway.is_none() {
            walk = walk.filter(|(_, cost)| *cost <= climb);
        }
        // A walkway only where it beats the walk and a new pillar: a climb
        // back up here and down again for the work, more for a taller one.
        let mut longest = if lays_from_here {
            (WALKWAY_CHAIN - chain).min(WALKWAY_LENGTH as usize) as i32
        } else {
            0
        };
        if !unreachable {
            longest = longest.min(climb / WALKWAY_TICKS);
        }
        if let Some((_, cost)) = walk {
            longest = longest.min((cost - 1) / WALKWAY_TICKS);
        }
        let way = match (longest > 0)
            .then(|| bridge::plan(ctx, job, body, task, &cells, longest, walkway.as_ref()))
            .flatten()
        {
            Some(path) => Way::Walkway(path),
            None => match walk {
                Some((s, _)) => Way::Walk(s),
                None => continue,
            },
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
        if matches!(way, Way::Walk(_)) && stance != top && plan::places(job, task) {
            let walls = plan::as_walls(ctx, job, &cells);
            match route::probe(ctx, stance, top, walls) {
                Some(Route::Open) => {}
                Some(_) => {
                    job.crew.deferrals.strike(task, stance);
                    continue;
                }
                None => return Some(Step::Plan),
            }
        }
        match way {
            Way::Walk(s) => {
                trace!(
                    "TRACE aloft: walking from {:?} to {s:?} for {task:?}",
                    body.cell
                );
                job.crew.aloft.aimed = Some((task, s));
                return Some(walk_to(ctx, s, Then::Course(task)));
            }
            Way::Walkway(path) => {
                trace!(
                    "TRACE aloft: walkway from {:?} along {path:?} for {task:?} (chain {chain})",
                    body.cell
                );
                job.crew.aloft.bridge = Some(match walkway {
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
                return Some(Step::Bridge {
                    since: ctx.now,
                    asked: 0,
                });
            }
        }
    }
    None
}
