use crate::host::prelude::*;

use super::tuning::reach::POCKET_REGION;
use super::{open_block, Ctx};
use crate::geometry::{manhattan, offset, FACES};
use crate::survey::Known;
use crate::worker::Job;

pub fn seals(ctx: &mut Ctx, job: &Job, i: usize) -> Option<Option<usize>> {
    let survey = job.survey.as_ref()?;
    let cells = job.design.cells(job.design.units[i]);
    let beside: Vec<[i32; 3]> = around(&cells)
        .into_iter()
        .filter(|n| {
            job.design.unit_at(*n).is_some_and(|u| {
                u != i && matches!(survey.known[u], Known::Place(_)) && !job.crew.built.contains(&u)
            })
        })
        .collect();
    if beside.is_empty() {
        return Some(None);
    }
    for (n, block) in beside.iter().zip(get_blocks(beside.clone())) {
        if passes(ctx, job, *n, block?)
            && closed_off(ctx, job, *n, &cells)?
            && !closed_off(ctx, job, *n, &[])?
        {
            return Some(job.design.unit_at(*n));
        }
    }
    Some(None)
}

pub fn opener(ctx: &mut Ctx, job: &Job, i: usize, near: [i32; 3], unseen: bool) -> Option<usize> {
    let survey = job.survey.as_ref()?;
    let unit = job.design.units[i];
    let cells = job.design.cells(unit);
    if !passes(ctx, job, unit.pos, get_block(unit.pos)?) {
        return None;
    }
    if !unseen && !closed_off(ctx, job, unit.pos, &[])? {
        return None;
    }
    let mut openers: Vec<([i32; 3], usize)> = around(&cells)
        .into_iter()
        .filter_map(|n| job.design.unit_at(n).map(|o| (n, o)))
        .filter(|(_, o)| {
            let unit = job.design.units[*o];
            matches!(survey.known[*o], Known::Satisfied)
                && matches!(job.design.plan(unit), crate::design::Plan::Build { .. })
                && job.design.cells(unit).len() == 1
                && !job.design.fragile(unit)
        })
        .collect();
    openers.sort_by_key(|(n, _)| {
        let dy = n[1] - unit.pos[1];
        (dy < 0, dy > 0, manhattan(*n, near))
    });
    for (n, o) in openers {
        let holds_fragile = FACES.iter().any(|f| {
            job.design
                .unit_at(offset(n, *f))
                .is_some_and(|u| u != i && job.design.fragile(job.design.units[u]))
        });
        if holds_fragile {
            continue;
        }
        let outward: Vec<[i32; 3]> = FACES
            .iter()
            .map(|f| offset(n, *f))
            .filter(|m| !cells.contains(m))
            .collect();
        for (m, block) in outward.iter().zip(get_blocks(outward.clone())) {
            if passes(ctx, job, *m, block?) && !closed_off(ctx, job, *m, &[])? {
                return Some(o);
            }
        }
    }
    None
}

fn closed_off(ctx: &mut Ctx, job: &Job, from: [i32; 3], filled: &[[i32; 3]]) -> Option<bool> {
    let mut region = vec![from];
    let mut frontier = vec![from];
    while !frontier.is_empty() {
        let mut next = around(&frontier);
        next.retain(|n| !filled.contains(n) && !region.contains(n));
        frontier.clear();
        for (cell, block) in next.iter().zip(get_blocks(next.clone())) {
            if passes(ctx, job, *cell, block?) {
                region.push(*cell);
                frontier.push(*cell);
            }
        }
        if region.len() > POCKET_REGION {
            return Some(false);
        }
    }
    Some(true)
}

fn around(cells: &[[i32; 3]]) -> Vec<[i32; 3]> {
    let mut out: Vec<[i32; 3]> = crate::geometry::beside(cells)
        .filter(|n| !cells.contains(n))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

fn passes(ctx: &mut Ctx, job: &Job, cell: [i32; 3], block: BlockId) -> bool {
    job.crew.scaffolding.cells.contains(&cell) || open_block(ctx, block)
}
