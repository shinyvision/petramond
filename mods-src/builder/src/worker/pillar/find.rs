use crate::host::prelude::*;

use super::Pillar;
use crate::content::GOLEM;
use crate::geometry::{feet_of, manhattan, offset, reaches, SIDES};
use crate::project::Project;
use crate::worker::route::{self, Hubs};
use crate::worker::stance::Search;
use crate::worker::step::Task;
use crate::worker::tuning::price::{LEVEL_MOVES, MOUNT_MOVES, UNWALKED};
use crate::worker::tuning::reach::{COLUMNS, MAX_HEIGHT, PILLAR_ROUTES};
use crate::worker::tuning::window::WEIGHED;
use crate::worker::Job;
use crate::worker::{open_block, plan, sight, Body, Ctx};

pub(in crate::worker) fn lays(
    job: &Job,
    body: &Body,
    perches: &[[i32; 3]],
    open: &[[i32; 3]],
    target: [i32; 3],
) -> Vec<i32> {
    let mut counts = vec![0; perches.len()];
    let mut near: Vec<[i32; 3]> = open
        .iter()
        .copied()
        .filter(|c| perches.iter().any(|p| reaches(feet_of(*p), &[*c])))
        .collect();
    near.sort_by_key(|c| manhattan(*c, target));
    near.truncate(WEIGHED);
    for cell in near {
        let Some(i) = job.design.unit_at(cell) else {
            continue;
        };
        let task = crate::worker::Task::Unit(i);
        if !plan::places(job, task) {
            continue;
        }
        let seen = sight::sees(body, perches, &[cell], &sight::work(job, task));
        for (count, refusal) in counts.iter_mut().zip(seen) {
            *count += i32::from(refusal.is_none());
        }
    }
    counts
}

pub fn find(
    ctx: &mut Ctx,
    job: &Job,
    project: &Project,
    body: &Body,
    task: Task,
    cells: &[[i32; 3]],
    open: &[[i32; 3]],
) -> Search<Pillar> {
    let Some(low) = cells.iter().min_by_key(|c| c[1]).copied() else {
        return Search::None;
    };
    let mut perches = perches_for(job, task, cells, low);
    if perches.is_empty() {
        return Search::None;
    }
    perches.sort_by_key(|p| {
        let away = manhattan([p[0], 0, p[2]], [body.cell[0], 0, body.cell[2]]);
        (
            away * 2 - covers(*p, open),
            job.design.contains_column(p[0], p[2]),
            -p[1],
        )
    });
    let viable = viable(ctx, job, project, body, task, cells, perches);
    let viable = by_trip(ctx, job, body, open, low, viable);
    choose(ctx, job, project, body, cells, viable)
}

fn perches_for(job: &Job, task: Task, cells: &[[i32; 3]], low: [i32; 3]) -> Vec<[i32; 3]> {
    let mut perches = Vec::new();
    for top in [
        low[1] - 1,
        low[1] - 2,
        low[1] - 3,
        low[1],
        low[1] + 1,
        low[1] + 2,
    ] {
        for dx in -4..=4 {
            for dz in -4..=4 {
                let perch = [low[0] + dx, top, low[2] + dz];
                let head = offset(perch, [0, 1, 0]);
                if cells.iter().any(|c| *c == perch || *c == head)
                    || job.crew.deferrals.blind(task, perch)
                    || job.design.governed.contains(&head)
                    || job.design.governed.contains(&perch)
                    || !reaches(feet_of(perch), cells)
                {
                    continue;
                }
                perches.push(perch);
            }
        }
    }
    perches
}

fn covers(perch: [i32; 3], open: &[[i32; 3]]) -> i32 {
    open.iter()
        .filter(|c| reaches(feet_of(perch), &[**c]))
        .count() as i32
}

fn viable(
    ctx: &mut Ctx,
    job: &Job,
    project: &Project,
    body: &Body,
    task: Task,
    cells: &[[i32; 3]],
    perches: Vec<[i32; 3]>,
) -> Vec<([i32; 3], i32)> {
    let sees = sight::sees(body, &perches, cells, &sight::work(job, task));
    let mut viable: Vec<([i32; 3], i32)> = Vec::new();
    for (perch, refusal) in perches.into_iter().zip(sees) {
        if viable.len() == COLUMNS * 3 {
            break;
        }
        if refusal.is_some() {
            continue;
        }
        if let Some(base) = column_base(ctx, job, project, [perch[0], perch[2]], perch[1], cells) {
            let foot = [perch[0], base, perch[2]];
            if !route::failed_recently(ctx, foot, project.home)
                && !viable.iter().any(|(p, b)| [p[0], *b, p[2]] == foot)
            {
                viable.push((perch, base));
            }
        }
    }
    viable
}

fn by_trip(
    ctx: &mut Ctx,
    job: &Job,
    body: &Body,
    open: &[[i32; 3]],
    low: [i32; 3],
    mut viable: Vec<([i32; 3], i32)>,
) -> Vec<([i32; 3], i32)> {
    viable.sort_by_key(|(p, b)| job.design.filled.contains(&[p[0], *b - 1, p[2]]));
    let walks: Option<Vec<Option<u32>>> =
        route::region(ctx, body.cell, false, &[])
            .flatten()
            .map(|region| {
                viable
                    .iter()
                    .map(|(p, b)| region.moves([p[0], *b, p[2]]))
                    .collect()
            });
    let Some(walks) = walks else {
        return viable;
    };
    let perches: Vec<[i32; 3]> = viable.iter().map(|(p, _)| *p).collect();
    let lays = lays(job, body, &perches, open, low);
    let mut keyed: Vec<(([i32; 3], i32), i32)> = viable
        .into_iter()
        .zip(walks)
        .zip(lays)
        .map(|(((perch, base), moves), lays)| {
            let on_course = job.design.filled.contains(&[perch[0], base - 1, perch[2]]);
            let trip = Trip {
                moves,
                on_course,
                levels: perch[1] - base,
                lays,
                covers: covers(perch, open),
            };
            ((perch, base), trip.key())
        })
        .collect();
    keyed.sort_by_key(|(_, key)| *key);
    keyed.into_iter().map(|(v, _)| v).collect()
}

struct Trip {
    moves: Option<u32>,
    on_course: bool,
    levels: i32,
    lays: i32,
    covers: i32,
}

impl Trip {
    fn key(&self) -> i32 {
        let walk = match self.moves {
            Some(m) => m as i32,
            None if self.on_course => UNWALKED * 4,
            None => UNWALKED,
        };
        let blocks = (4 * self.lays + self.covers).max(4);
        let trip = walk + LEVEL_MOVES * self.levels + MOUNT_MOVES;
        trip * 64 / blocks
    }
}

fn choose(
    ctx: &mut Ctx,
    job: &Job,
    project: &Project,
    body: &Body,
    cells: &[[i32; 3]],
    mut viable: Vec<([i32; 3], i32)>,
) -> Search<Pillar> {
    viable.truncate(COLUMNS);
    let mut asks = Vec::new();
    for (perch, base) in &viable {
        let foot = [perch[0], *base, perch[2]];
        asks.push(foot);
        asks.extend(SIDES.iter().map(|s| offset(foot, *s)));
    }
    let standing = if asks.is_empty() {
        Vec::new()
    } else {
        footholds(GOLEM, asks.clone())
    };
    let mut routed = 0;
    let mut rejected = Vec::new();
    for (i, (perch, base)) in viable.into_iter().enumerate() {
        let at = i * 5;
        if !standing[at] {
            rejected.push(([perch[0], base, perch[2]], "no standing"));
            continue;
        }
        let foot = [perch[0], base, perch[2]];
        let exit = (1..5)
            .find(|k| standing[at + k])
            .map_or(foot, |k| asks[at + k]);
        let pillar = Pillar {
            column: [perch[0], perch[2]],
            base,
            top: perch[1],
            exit,
            onward: None,
        };
        if foot == body.cell {
            return Search::Found(pillar);
        }
        if routed == PILLAR_ROUTES {
            break;
        }
        routed += 1;
        let hubs = Hubs::new(project.home, &job.crew.trail);
        match route::round_trip(ctx, hubs, foot) {
            Some(true) => return Search::Found(pillar),
            Some(false) => rejected.push((foot, "no round trip")),
            None => return Search::Busy,
        }
    }
    trace!(
        "TRACE pillar for {cells:?}: none of {} viable perches {rejected:?}",
        asks.len() / 5
    );
    Search::None
}

pub(super) fn column_base(
    ctx: &mut Ctx,
    job: &Job,
    project: &Project,
    column: [i32; 2],
    top: i32,
    cells: &[[i32; 3]],
) -> Option<i32> {
    let ys: Vec<i32> = ((top - MAX_HEIGHT)..=(top + 1)).rev().collect();
    let positions: Vec<[i32; 3]> = ys.iter().map(|y| [column[0], *y, column[1]]).collect();
    let blocks = get_blocks(positions.clone());
    for (cell, block) in positions.into_iter().zip(blocks) {
        if open_block(ctx, block?) {
            if job.design.governed.contains(&cell)
                || cells.contains(&cell)
                || project.scaffolds.contains(&cell)
            {
                return None;
            }
            continue;
        }
        let base = cell[1] + 1;
        return (base < top).then_some(base);
    }
    None
}

#[cfg(test)]
mod tests;
