//! Work that would wall the golem in, or wall ground off, waits its turn.

use mod_sdk::*;

use super::places;
use crate::geometry::{offset, SIDES};
use crate::jobs::Job;
use crate::survey::Known;
use crate::worker::body::stands_at;
use crate::worker::crew::Deferrals;
use crate::worker::route::{self, Hubs};
use crate::worker::tuning::patience::{CUTTER_ROUNDS, SITE_STALLED};
use crate::worker::{Ctx, Task, TRACE};

/// Whether placing `task` would cut the golem at `from` off from its home.
/// Clearance never seals, and neither does a unit with nowhere to walk.
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

/// Whether placing `task` would leave the golem standing at `from` no way home:
/// a fault of the stance, not of the placement.
pub(super) fn strands(
    ctx: &mut Ctx,
    job: &Job,
    project: &crate::project::Project,
    from: [i32; 3],
    task: Task,
    cells: &[[i32; 3]],
) -> Option<bool> {
    // A support scaffold walls the golem in as surely as a block does.
    if !places(job, task) && !matches!(task, Task::Support { .. }) {
        return Some(false);
    }
    // Nobody stands in a cell that is no foothold now (a pillar's own foot
    // while it stands): it strands nobody.
    if !stands_at(from) {
        return Some(false);
    }
    let walls = as_walls(ctx, job, cells);
    let hubs = Hubs::new(project.home, &job.crew.trail);
    Some(route::out(ctx, hubs, from, &walls)? != Route::Open)
}

/// Whether `task` walls off ground with work beside it and must wait its turn.
pub(in crate::worker) fn cutting(
    ctx: &mut Ctx,
    job: &mut Job,
    project: &crate::project::Project,
    task: Task,
    cells: &[[i32; 3]],
) -> Option<bool> {
    // A door is opened as it is laid, and the golem walks through it open.
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
        let waits = Deferrals::round(&mut job.crew.deferrals.support_waits, unit, CUTTER_ROUNDS);
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
        // Ways in are held apart; a wall that keeps cutting off some ledge
        // with work beside it goes in after waiting a few rounds, or a ring
        // of such walls waits on itself forever.
        let waits = Deferrals::round(&mut job.crew.deferrals.cutter_waits, i, CUTTER_ROUNDS);
        return Some(waits && !only_cutting_work_left(ctx, job, Some(i)));
    }
    Some(false)
}

/// Whether building `cells` would wall off ground the golem reaches from
/// home now — a door closing a house, the last block of a room. Only cells
/// with standing room on two sides can be such a passage.
fn cuts_off(
    ctx: &mut Ctx,
    hubs: Hubs,
    cells: &[[i32; 3]],
    walls: &[[i32; 3]],
    needed: impl Fn([i32; 3]) -> bool,
) -> Option<bool> {
    // Standing room beside the cells, a step up or down included (a doorway
    // often sits a step above the ground outside it), and two down: a block at
    // head height closes the gap under it as surely as one at the feet.
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
    // The site's floods toward home answer exactly: ground beside the cells
    // that reaches home now and would not once they stand is cut off. Ground
    // that does not reach home now (a roof course) is cut off from nothing.
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
    // Asked outward from each side: from inside a room the search floods the
    // room and is decided quickly, where a search in from home is not.
    for n in standing {
        let after = route::out(ctx, hubs, n, walls)?;
        if after == Route::Open {
            continue;
        }
        // Undecided now reads as reachable: a wrong guess only makes the unit
        // wait for the end of the build.
        let before = route::out(ctx, hubs, n, &[])?;
        trace!("TRACE seal {cells:?} from {n:?}: after {after:?} before {before:?}");
        // Ground nothing is left to do from (a roof course's ledge) may close.
        if before != Route::Closed && needed(n) {
            return Some(true);
        }
    }
    Some(false)
}

/// The cells a route judges `cells` by once they stand: blocks there, and the
/// cell above one no body stands on or steps over (a fence, a pane, a wall),
/// since a route takes a stand-in block for a cube it can climb onto.
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

/// Whether a way in is a door: opened as it is laid and walked through open,
/// it closes nothing off and need not wait for the end of the build.
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

/// Whether open work other than `unit` is near standing at `n`: within two
/// reaches, so work across the room that ground opens onto counts too.
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

/// Whether every open unit but `except` walls off ground, is a way in, or
/// was built already, or nothing has landed for a long while: what closes
/// the site may go in now.
pub(super) fn only_cutting_work_left(ctx: &Ctx, job: &Job, except: Option<usize>) -> bool {
    // A build this long without a block landing is stuck on something else, and
    // closing a gap may be all that is left. Only as a last resort: a front
    // door laid in an ordinary stall seals the house with interior work still
    // open.
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
