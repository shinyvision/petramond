use crate::host::prelude::*;

use super::route::{self, Hubs};
use super::tuning::reach::{FOOTHOLDS, NEAR_WORK, STANCE_ROUTES};
use super::{Body, Ctx};
use crate::content::GOLEM;
use crate::geometry::{feet_of, manhattan, offset, reaches};

pub enum Search<T> {
    Found(T),
    Busy,
    None,
    Unseen,
}

pub fn find(
    ctx: &mut Ctx,
    body: &Body,
    hubs: Hubs,
    cells: &[[i32; 3]],
    work: &super::sight::Work,
    off_top: bool,
    usable: impl Fn([i32; 3]) -> bool,
) -> Search<[i32; 3]> {
    let target = cells[0];
    let mut candidates = Vec::new();
    for dy in -5..=2 {
        for dx in -4..=4 {
            for dz in -4..=4 {
                let s = offset(target, [dx, dy, dz]);
                let head = offset(s, [0, 1, 0]);
                if cells.iter().any(|c| *c == s || *c == head) {
                    continue;
                }
                if off_top && cells.contains(&offset(s, [0, -1, 0])) {
                    continue;
                }
                if reaches(feet_of(s), cells) && usable(s) {
                    candidates.push(s);
                }
            }
        }
    }
    let mut near_work = candidates.clone();
    near_work.sort_by_key(|s| manhattan(*s, target));
    candidates.sort_by_key(|s| (manhattan(*s, body.cell), (s[1] - body.cell[1]).abs()));
    candidates.truncate(FOOTHOLDS);
    let extra: Vec<[i32; 3]> = near_work
        .into_iter()
        .filter(|s| !candidates.contains(s))
        .take(NEAR_WORK)
        .collect();
    candidates.extend(extra);
    let moves: Option<Vec<u32>> =
        route::region(ctx, body.cell, false, &[])
            .flatten()
            .map(|region| {
                candidates
                    .iter()
                    .map(|s| region.moves(*s).unwrap_or(u32::MAX))
                    .collect()
            });
    match moves {
        Some(moves) => {
            let mut keyed: Vec<([i32; 3], (u32, i32))> = candidates
                .into_iter()
                .zip(moves)
                .map(|(s, moves)| (s, (moves, manhattan(s, body.cell))))
                .collect();
            keyed.sort_by_key(|(_, key)| *key);
            candidates = keyed.into_iter().map(|(s, _)| s).collect();
        }
        None => candidates.sort_by_key(|s| (manhattan(*s, body.cell), (s[1] - body.cell[1]).abs())),
    }
    let standing = footholds(GOLEM, candidates.clone());
    let candidates: Vec<[i32; 3]> = candidates
        .into_iter()
        .zip(standing)
        .filter_map(|(s, ok)| ok.then_some(s))
        .collect();
    let sees = super::sight::sees(body, &candidates, cells, work);
    let mut routed = 0;
    let mut tried = Vec::new();
    let standing_count = candidates.len();
    let mut seen_any = false;
    for (s, refusal) in candidates.into_iter().zip(sees) {
        if refusal.is_some() {
            continue;
        }
        seen_any = true;
        if s == body.cell {
            return Search::Found(s);
        }
        if route::failed_recently(ctx, s, hubs.home) {
            continue;
        }
        if routed == STANCE_ROUTES {
            break;
        }
        routed += 1;
        match route::round_trip(ctx, hubs, s) {
            Some(true) => return Search::Found(s),
            Some(false) => tried.push(s),
            None => return Search::Busy,
        }
    }
    trace!(
        "TRACE stance for {cells:?} from {:?}: {standing_count} standing, routes failed to {tried:?}",
        body.cell
    );
    if standing_count > 0 && !seen_any {
        return Search::Unseen;
    }
    Search::None
}
