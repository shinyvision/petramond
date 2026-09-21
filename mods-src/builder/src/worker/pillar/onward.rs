//! A pillar whose top walks on along a roof course to work no perch sees.

use mod_sdk::*;

use super::find::column_base;
use super::Pillar;
use crate::content::GOLEM;
use crate::fx::HashSet;
use crate::geometry::{feet_of, manhattan, offset, reaches, SIDES};
use crate::jobs::Job;
use crate::project::Project;
use crate::worker::route::{self, Hubs};
use crate::worker::stance::Search;
use crate::worker::step::Task;
use crate::worker::tuning::reach::{ONWARD_SPAN, ONWARD_STANCES, ONWARD_TOPS};
use crate::worker::{sight, Body, Ctx};

/// Where the standing room level with `stance` (a roof course, a wall top)
/// runs out, nearest first: open cells beside it a pillar could top out in.
fn walkway_ends(job: &Job, stance: [i32; 3], span: i32) -> Vec<[i32; 3]> {
    let mut walkway: HashSet<[i32; 3]> = [stance].into_iter().collect();
    let mut frontier = vec![stance];
    let mut edge = Vec::new();
    let mut ends = Vec::new();
    while !frontier.is_empty() {
        let mut next: Vec<[i32; 3]> = Vec::new();
        for cell in &frontier {
            for side in SIDES {
                let n = offset(*cell, side);
                if manhattan(n, stance) <= span
                    && !walkway.contains(&n)
                    && !next.contains(&n)
                    && !edge.contains(&n)
                {
                    next.push(n);
                }
            }
        }
        let standing = crate::design::paged(next.clone(), |cells| footholds(GOLEM, cells));
        frontier.clear();
        for (cell, ok) in next.into_iter().zip(standing) {
            if ok {
                walkway.insert(cell);
                frontier.push(cell);
            } else {
                edge.push(cell);
                if !job.design.governed.contains(&cell)
                    && !job.design.governed.contains(&offset(cell, [0, 1, 0]))
                {
                    ends.push(cell);
                }
            }
        }
    }
    // A cell or two further too: a pillar top may stand clear of an overhang
    // (an eave row the design governs) and step across to the walkway.
    let mut ring = edge.clone();
    for _ in 0..2 {
        let beyond: Vec<[i32; 3]> = ring
            .iter()
            .flat_map(|e| SIDES.iter().map(move |s| offset(*e, *s)))
            .filter(|c| !walkway.contains(c) && !edge.contains(c) && !ends.contains(c))
            .collect();
        ring.clear();
        for cell in beyond {
            if ring.contains(&cell) {
                continue;
            }
            ring.push(cell);
            if !job.design.governed.contains(&cell)
                && !job.design.governed.contains(&offset(cell, [0, 1, 0]))
            {
                ends.push(cell);
            }
        }
    }
    ends.sort_by_key(|c| manhattan(*c, stance));
    ends
}

/// A pillar whose top walks on to standing room that sees `cells`, for work
/// no perch sees: the middle of a ridge is reached along the roof course
/// beside it from a pillar at the gable, never from the ground.
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
                // Never on the work itself (a dig drops the stance, and no
                // face is seen from on top), nor where it was refused before.
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
    let hubs = Hubs::new(project.home, &job.crew.trail);
    for stance in stances {
        let tops = walkway_ends(job, stance, ONWARD_SPAN);
        let mut tried = 0;
        for top in tops {
            if tried == ONWARD_TOPS {
                break;
            }
            let Some(base) = column_base(ctx, job, project, [top[0], top[2]], top[1], cells) else {
                continue;
            };
            let foot = [top[0], base, top[2]];
            if job.design.filled.contains(&offset(foot, [0, -1, 0]))
                || route::failed_recently(ctx, foot, project.home)
            {
                continue;
            }
            tried += 1;
            let column: Vec<[i32; 3]> = (base..top[1]).map(|y| [top[0], y, top[2]]).collect();
            let Some(out) = route::probe(ctx, top, stance, column.clone()) else {
                return Search::Busy;
            };
            if out != Route::Open {
                continue;
            }
            let Some(back) = route::probe(ctx, stance, top, column) else {
                return Search::Busy;
            };
            if back != Route::Open {
                continue;
            }
            let asks: Vec<[i32; 3]> = std::iter::once(foot)
                .chain(SIDES.iter().map(|s| offset(foot, *s)))
                .collect();
            let standing = footholds(GOLEM, asks.clone());
            let exit = (1..5).find(|k| standing[*k]).map_or(foot, |k| asks[k]);
            if !standing[0] {
                continue;
            }
            match route::round_trip(ctx, hubs, foot) {
                None => return Search::Busy,
                Some(false) => continue,
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
    }
    Search::None
}
