use crate::host::prelude::*;

use super::find::column_base;
use super::Pillar;
use crate::content::GOLEM;
use crate::fx::HashSet;
use crate::geometry::{feet_of, manhattan, offset, reaches, SIDES};
use crate::project::Project;
use crate::worker::route::{self, Hubs};
use crate::worker::stance::Search;
use crate::worker::step::Task;
use crate::worker::tuning::reach::{MAX_HEIGHT, ONWARD_COLUMNS, ONWARD_STANCES, ONWARD_TOPS};
use crate::worker::Job;
use crate::worker::{sight, Body, Ctx};

/// Whether a column down from `top` rises through a cell the design governs before it rests on
/// finished work.
fn through_design(job: &Job, top: [i32; 3]) -> bool {
    for y in (top[1] - MAX_HEIGHT..=top[1]).rev() {
        let cell = [top[0], y, top[2]];
        if job.design.filled.contains(&cell) {
            return false;
        }
        if job.design.governed.contains(&cell) {
            return true;
        }
    }
    false
}

/// Open cells one step off the ground that walks to `stance`, nearest walk first: where a pillar
/// from below lands the worker on that ground, whatever its shape (a flat walkway, a stair roof).
fn ways_up(ctx: &mut Ctx, job: &Job, stance: [i32; 3]) -> Option<Vec<[i32; 3]>> {
    let Some(region) = route::region(ctx, stance, true, &[])? else {
        return Some(Vec::new());
    };
    let mut ends: Vec<([i32; 3], u32)> = Vec::new();
    let mut seen: HashSet<[i32; 3]> = HashSet::default();
    for cell in region.cells() {
        let Some(moves) = region.moves(cell) else {
            continue;
        };
        for side in SIDES {
            let top = offset(cell, side);
            // A column landing on ground that already walks there lifts the worker nowhere new.
            if region.contains(top)
                || job.design.filled.contains(&top)
                || (top[1] - MAX_HEIGHT..top[1]).any(|y| region.contains([top[0], y, top[2]]))
                || job.design.governed.contains(&offset(top, [0, 1, 0]))
                || through_design(job, top)
                || !seen.insert(top)
            {
                continue;
            }
            ends.push((top, moves));
        }
    }
    ends.sort_by_key(|(top, moves)| (*moves, manhattan(*top, stance)));
    trace!(
        "TRACE ways up to {stance:?}: {} cells walk there, {} columns",
        region.cells().count(),
        ends.len()
    );
    Some(ends.into_iter().map(|(top, _)| top).collect())
}

pub fn find_onward(
    ctx: &mut Ctx,
    job: &Job,
    project: &Project,
    body: &Body,
    task: Task,
    cells: &[[i32; 3]],
) -> Search<Pillar> {
    let Some(target) = cells.first().copied() else {
        return Search::None;
    };
    let filled = &job.design.filled;
    let mut stances = Vec::new();
    for dy in -2..=1 {
        for dx in -3..=3 {
            for dz in -3..=3 {
                let s = offset(target, [dx, dy, dz]);
                if !cells.contains(&s)
                    && !cells.contains(&offset(s, [0, 1, 0]))
                    && !cells.contains(&offset(s, [0, -1, 0]))
                    && !job.crew.deferrals.blind(task, s)
                    && !filled.contains(&s)
                    && !filled.contains(&offset(s, [0, 1, 0]))
                    && reaches(feet_of(s), cells)
                {
                    stances.push(s);
                }
            }
        }
    }
    stances.sort_by_key(|s| manhattan(*s, target));
    let standing = footholds(GOLEM, stances.clone());
    let stances: Vec<[i32; 3]> = stances
        .into_iter()
        .zip(standing)
        .filter_map(|(s, ok)| ok.then_some(s))
        .collect();
    let sees = sight::sees(body, &stances, cells, &sight::work(job, task));
    let stances: Vec<[i32; 3]> = stances
        .into_iter()
        .zip(sees)
        .filter_map(|(s, refusal)| refusal.is_none().then_some(s))
        .take(ONWARD_STANCES)
        .collect();
    for stance in stances {
        match up_to(ctx, job, project, cells, stance) {
            Search::None | Search::Unseen => {}
            found => return found,
        }
    }
    Search::None
}

/// A pillar from ground walkable from home to where the worker steps off onto ground that walks
/// to `stance`.
pub(super) fn up_to(
    ctx: &mut Ctx,
    job: &Job,
    project: &Project,
    cells: &[[i32; 3]],
    stance: [i32; 3],
) -> Search<Pillar> {
    let hubs = Hubs::new(project.home, &job.crew.trail);
    let Some(tops) = ways_up(ctx, job, stance) else {
        return Search::Busy;
    };
    let mut tried = 0;
    let mut why: Vec<([i32; 3], &str)> = Vec::new();
    for top in tops.into_iter().take(ONWARD_COLUMNS) {
        if tried == ONWARD_TOPS {
            break;
        }
        if job
            .crew
            .scaffolding
            .refused
            .holds(&[top[0], top[2]], ctx.now)
        {
            why.push((top, "refused"));
            continue;
        }
        let Some(base) = column_base(ctx, job, project, [top[0], top[2]], top[1], cells) else {
            why.push((top, "no base"));
            continue;
        };
        let foot = [top[0], base, top[2]];
        if route::failed_recently(ctx, foot, project.home) {
            why.push((top, "failed recently"));
            continue;
        }
        tried += 1;
        let column: Vec<[i32; 3]> = (base..top[1]).map(|y| [top[0], y, top[2]]).collect();
        let Some(out) = route::probe(ctx, top, stance, column.clone()) else {
            return Search::Busy;
        };
        if out != Route::Open {
            why.push((top, "no walk on"));
            continue;
        }
        let Some(back) = route::probe(ctx, stance, top, column) else {
            return Search::Busy;
        };
        if back != Route::Open {
            why.push((top, "no walk back"));
            continue;
        }
        let asks: Vec<[i32; 3]> = std::iter::once(foot)
            .chain(SIDES.iter().map(|s| offset(foot, *s)))
            .collect();
        let standing = footholds(GOLEM, asks.clone());
        let exit = (1..5).find(|k| standing[*k]).map_or(foot, |k| asks[k]);
        if !standing[0] {
            why.push((top, "no foothold"));
            continue;
        }
        match route::round_trip(ctx, hubs, foot) {
            None => return Search::Busy,
            Some(false) => {
                why.push((top, "no round trip"));
                continue;
            }
            Some(true) => {}
        }
        return Search::Found(Pillar {
            column: [top[0], top[2]],
            base,
            top: top[1],
            exit,
            onward: Some(stance),
        });
    }
    trace!("TRACE no way up to {stance:?}: {why:?}");
    Search::None
}
