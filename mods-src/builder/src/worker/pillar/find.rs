//! Choosing where a pillar goes: the perch that lays the most for the
//! least walking and climbing.

use mod_sdk::*;

use super::Pillar;
use crate::content::GOLEM;
use crate::geometry::{feet_of, manhattan, offset, reaches, SIDES};
use crate::jobs::Job;
use crate::project::Project;
use crate::worker::route::{self, Hubs};
use crate::worker::stance::Search;
use crate::worker::step::Task;
use crate::worker::tuning::price::{LEVEL_MOVES, MOUNT_MOVES, UNWALKED};
use crate::worker::tuning::reach::{COLUMNS, MAX_HEIGHT, PILLAR_ROUTES};
use crate::worker::tuning::window::WEIGHED;
use crate::worker::{open_block, plan, sight, Body, Ctx};

/// How many blocks of the `open` work the golem would lay from each of
/// `perches` as the world stands: seen, against a face turned to it, and
/// turned the way it would be looking. Reach alone flatters a perch — half of
/// what is within arm's length of a pillar beside a roof faces away from it.
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

/// A pillar to reach `cells` from, preferring perches that also reach the
/// most of the other `open` work, so one climb does a stretch of wall.
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
    // A perch level with the target at head height first, then lower ones
    // (under a floor being laid the head must stay below it), then higher
    // ones reaching down (a wall under an eave is reached from above it).
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
                // A perch the task was already refused from would be
                // climbed and left again forever.
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
    if perches.is_empty() {
        return Search::None;
    }
    let design = &job.design;
    // Nearest the golem first, those reaching more work between equals: the
    // few places on the list go to columns beside it.
    perches.sort_by_key(|p| {
        let covers = open.iter().filter(|c| reaches(feet_of(*p), &[**c])).count() as i32;
        let away = manhattan([p[0], 0, p[2]], [body.cell[0], 0, body.cell[2]]);
        (away * 2 - covers, design.contains_column(p[0], p[2]), -p[1])
    });
    let sees = sight::sees(body, &perches, cells, &sight::work(job, task));
    let mut viable: Vec<([i32; 3], i32)> = Vec::new();
    for (perch, refusal) in perches.into_iter().zip(sees) {
        if viable.len() == COLUMNS * 3 {
            break;
        }
        if refusal.is_some() {
            continue;
        }
        // A foot already known cut off, or already listed for another
        // height, must not take one of the few places.
        if let Some(base) = column_base(ctx, job, project, [perch[0], perch[2]], perch[1], cells) {
            let foot = [perch[0], base, perch[2]];
            if !route::failed_recently(ctx, foot, project.home)
                && !viable.iter().any(|(p, b)| [p[0], *b, p[2]] == foot)
            {
                viable.push((perch, base));
            }
        }
    }
    // Feet on the ground first when no flood says otherwise: a base on the
    // design's own courses (a roof) is rarely walkable, and ranked by the work
    // it covers it would crowd out every pillar that can be climbed.
    viable.sort_by_key(|(p, b)| job.design.filled.contains(&[p[0], *b - 1, p[2]]));
    // Within that, work reached against the walk to the foot and the climb:
    // the column across the house from the golem is the whole way round.
    let walks: Option<Vec<Option<u32>>> =
        route::region(ctx, body.cell, false, &[])
            .flatten()
            .map(|region| {
                viable
                    .iter()
                    .map(|(p, b)| region.moves([p[0], *b, p[2]]))
                    .collect()
            });
    if let Some(walks) = walks {
        let perches: Vec<[i32; 3]> = viable.iter().map(|(p, _)| *p).collect();
        let lays = lays(job, body, &perches, open, low);
        let mut keyed: Vec<(([i32; 3], i32), i32)> = viable
            .into_iter()
            .zip(walks)
            .zip(lays)
            .map(|(((perch, base), moves), lays)| {
                let covers = open
                    .iter()
                    .filter(|c| reaches(feet_of(perch), &[**c]))
                    .count() as i32;
                // A foot on the design's own courses that the flood reaches
                // (the upper floor) is as good as ground: a short pillar from
                // it beats the long way out.
                let on_course = job.design.filled.contains(&[perch[0], base - 1, perch[2]]);
                let walk = match moves {
                    Some(m) => m as i32,
                    None if on_course => UNWALKED * 4,
                    None => UNWALKED,
                };
                // What it would lay now counts in full; what is merely in
                // reach may follow once a neighbour stands, or may never be
                // seen from here at all.
                let blocks = (4 * lays + covers).max(4);
                let trip = walk + LEVEL_MOVES * (perch[1] - base) + MOUNT_MOVES;
                ((perch, base), trip * 64 / blocks)
            })
            .collect();
        keyed.sort_by_key(|(_, key)| *key);
        viable = keyed.into_iter().map(|(v, _)| v).collect();
    }
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
        // Taken down, the pillar leaves the golem on its own foot; ground
        // beside it at the same height is only a preference (a slope has none).
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

/// The feet level a pillar in `column` would start from: the first open
/// cell above ground, with open cells all the way up past `top` and none of
/// them governed by the design.
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
