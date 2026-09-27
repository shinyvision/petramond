use crate::host::prelude::*;

use super::places;
use crate::geometry::{offset, SIDES};
use crate::survey::Known;
use crate::worker::body::stands_at;
use crate::worker::route::{self, Hubs};
use crate::worker::tuning::patience::{CUTTER_ROUNDS, SITE_STALLED};
use crate::worker::Job;
use crate::worker::{Ctx, Task, TRACE};

pub(super) fn sealed_by(
    ctx: &mut Ctx,
    job: &mut Job,
    project: &crate::project::Project,
    from: [i32; 3],
    task: Task,
    cells: &[[i32; 3]],
) -> Option<bool> {
    Some(strands(ctx, job, project, from, task, cells)? || cutting(ctx, job, project, task, cells)?)
}

pub(super) fn strands(
    ctx: &mut Ctx,
    job: &Job,
    project: &crate::project::Project,
    from: [i32; 3],
    task: Task,
    cells: &[[i32; 3]],
) -> Option<bool> {
    if !places(job, task) && !matches!(task, Task::Support { .. }) {
        return Some(false);
    }
    if !stands_at(from) {
        return Some(false);
    }
    let walls = as_walls(ctx, job, cells);
    let hubs = Hubs::new(project.home, &job.crew.trail);
    Some(route::out(ctx, hubs, from, &walls)? != Route::Open)
}

pub(in crate::worker) fn cutting(
    ctx: &mut Ctx,
    job: &mut Job,
    project: &crate::project::Project,
    task: Task,
    cells: &[[i32; 3]],
) -> Option<bool> {
    if let Task::Unit(i) = task {
        if job.design.passage(i) && swings_open(ctx.caches, &job.design, i) {
            return Some(false);
        }
    }
    let trail = job.crew.trail.clone();
    let hubs = Hubs::new(project.home, &trail);
    let walls = as_walls(ctx, job, cells);
    // A support column raised inside a room cuts it off like a wall would;
    // it waits for another way to hold its unit up a few rounds, as a wall
    // does, then goes up: supports for a pair of units can each wait on the
    // other's ground forever.
    if let Task::Support { unit, .. } = task {
        if !cuts_off(ctx, hubs, cells, &walls, |n| work_near(job, unit, n))? {
            return Some(false);
        }
        let waits = job.crew.deferrals.support_waits.within(unit, CUTTER_ROUNDS);
        return Some(waits && !only_cutting_work_left(ctx, job, Some(unit)));
    }
    let Task::Unit(i) = task else {
        return Some(false);
    };
    if !places(job, task) {
        return Some(false);
    }
    if cuts_off(ctx, hubs, cells, &walls, |n| work_near(job, i, n))? {
        job.crew.deferrals.cutters.insert(i);
        let waits = job.crew.deferrals.cutter_waits.within(i, CUTTER_ROUNDS);
        return Some(waits && !only_cutting_work_left(ctx, job, Some(i)));
    }
    Some(false)
}

fn cuts_off(
    ctx: &mut Ctx,
    hubs: Hubs,
    cells: &[[i32; 3]],
    walls: &[[i32; 3]],
    needed: impl Fn([i32; 3]) -> bool,
) -> Option<bool> {
    let mut beside = Vec::new();
    for cell in cells {
        for side in SIDES {
            for step in [0, -1, 1, -2] {
                let n = offset(*cell, [side[0], step, side[2]]);
                if !cells.contains(&n) && !beside.contains(&n) {
                    beside.push(n);
                }
            }
        }
    }
    let standing: Vec<[i32; 3]> = beside
        .iter()
        .zip(footholds(crate::content::GOLEM, beside.clone()))
        .filter_map(|(n, ok)| ok.then_some(*n))
        .collect();
    if standing.len() < 2 {
        return Some(false);
    }
    let reaching: Option<Vec<[i32; 3]>> = route::region(ctx, hubs.home, true, &[])?.map(|now| {
        standing
            .iter()
            .copied()
            .filter(|n| now.contains(*n))
            .collect()
    });
    if let Some(reaching) = reaching {
        if reaching.len() < 2 {
            return Some(false);
        }
        if let Some(after) = route::region(ctx, hubs.home, true, walls)? {
            let cut = reaching
                .iter()
                .copied()
                .find(|n| !after.contains(*n) && needed(*n));
            if let (true, Some(n)) = (TRACE, cut) {
                log(&format!("TRACE seal {cells:?} cuts {n:?} off from home"));
            }
            return Some(cut.is_some());
        }
    }
    for n in standing {
        let after = route::out(ctx, hubs, n, walls)?;
        if after == Route::Open {
            continue;
        }
        let before = route::out(ctx, hubs, n, &[])?;
        trace!("TRACE seal {cells:?} from {n:?}: after {after:?} before {before:?}");
        if before != Route::Closed && needed(n) {
            return Some(true);
        }
    }
    Some(false)
}

pub(in crate::worker) fn as_walls(ctx: &mut Ctx, job: &Job, cells: &[[i32; 3]]) -> Vec<[i32; 3]> {
    let mut walls = cells.to_vec();
    for cell in cells {
        let barrier = job
            .design
            .unit_at(*cell)
            .map(|u| {
                job.design.records[job.design.units[u].record as usize]
                    .block
                    .clone()
            })
            .and_then(|name| {
                ctx.caches.block_named(&name).map(|info| {
                    let whole = |lo: f32, hi: f32| lo <= 0.01 && hi >= 0.99;
                    !info.collision.is_empty()
                        && info
                            .collision
                            .iter()
                            .all(|(lo, hi)| !(whole(lo[0], hi[0]) && whole(lo[2], hi[2])))
                        || info.collision.iter().any(|(_, hi)| hi[1] > 1.0)
                })
            })
            .unwrap_or(false);
        let above = offset(*cell, [0, 1, 0]);
        if barrier && !walls.contains(&above) {
            walls.push(above);
        }
    }
    walls
}

pub(super) fn swings_open(
    caches: &mut crate::caches::Caches,
    design: &crate::design::Design,
    unit: usize,
) -> bool {
    let block = &design.records[design.units[unit].record as usize].block;
    caches
        .block_named(block)
        .is_some_and(|info| info.interaction == Some(BlockUse::ToggleDoor))
}

fn work_near(job: &Job, unit: usize, n: [i32; 3]) -> bool {
    let Some(survey) = job.survey.as_ref() else {
        return true;
    };
    let r = 2 * crate::geometry::PLAN_REACH.ceil() as i32;
    (-r..=r).any(|dx| {
        (-r..=r).any(|dy| {
            (-r..=r).any(|dz| {
                job.design
                    .unit_at(offset(n, [dx, dy, dz]))
                    .is_some_and(|j| {
                        j != unit && survey.known[j].open() && !job.crew.built.contains(&j)
                    })
            })
        })
    })
}

pub(super) fn only_cutting_work_left(ctx: &Ctx, job: &Job, except: Option<usize>) -> bool {
    if ctx.now > job.crew.pace.progress_at + SITE_STALLED {
        return true;
    }
    let Some(survey) = job.survey.as_ref() else {
        return false;
    };
    survey
        .known
        .iter()
        .enumerate()
        .skip(job.crew.pace.cursor)
        .filter(|(i, k)| Some(*i) != except && matches!(k, Known::Place(_) | Known::Clear { .. }))
        .all(|(i, _)| {
            job.crew.deferrals.cutters.contains(&i)
                || job.crew.built.contains(&i)
                || job.design.passage(i)
                || job.design.glazing(i)
        })
}
