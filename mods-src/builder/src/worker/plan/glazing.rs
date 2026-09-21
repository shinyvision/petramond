//! Windows are glazed last: an open panel is a way through until then.

use mod_sdk::*;

use crate::fx::HashSet;
use crate::geometry::{offset, SIDES};
use crate::jobs::Job;
use crate::survey::Known;
use crate::worker::tuning::window::SCAN;

/// The open panels (panes, railings) whose gap is a way through as the world
/// stands: standing room level with it or a step down on two opposite sides.
/// Only those are worth leaving open to the end; an upper window over a drop
/// is no way anywhere, and left for last it is a climb of its own.
pub(super) fn ways_through(job: &Job, ceiling: Option<i32>) -> HashSet<usize> {
    let Some(survey) = job.survey.as_ref() else {
        return HashSet::default();
    };
    let panels: Vec<usize> = survey
        .known
        .iter()
        .enumerate()
        .skip(job.crew.pace.cursor)
        .take(SCAN)
        .filter(|(i, known)| {
            matches!(known, Known::Place(_))
                && job.design.glazing(*i)
                && ceiling.is_none_or(|top| job.design.units[*i].pos[1] <= top)
        })
        .map(|(i, _)| i)
        .collect();
    // Per panel, per side: level with it, then a step down.
    let asks: Vec<[i32; 3]> = panels
        .iter()
        .flat_map(|i| {
            let pos = job.design.units[*i].pos;
            SIDES
                .iter()
                .flat_map(move |side| [0, -1].map(|dy| offset(pos, [side[0], dy, side[2]])))
        })
        .collect();
    let standing = crate::design::paged(asks, |cells| footholds(crate::content::GOLEM, cells));
    panels
        .into_iter()
        .zip(standing.chunks(8))
        .filter(|(_, around)| {
            let side = |n: usize| around[2 * n] || around[2 * n + 1];
            let opposite = |a: [i32; 3], b: [i32; 3]| a[0] == -b[0] && a[2] == -b[2];
            (0..4).any(|a| (a + 1..4).any(|b| opposite(SIDES[a], SIDES[b]) && side(a) && side(b)))
        })
        .map(|(i, _)| i)
        .collect()
}

/// Whether every open placement left is a window pane (or built already):
/// the glazing goes in.
pub(super) fn only_glazing_left(
    job: &Job,
    held: Option<&std::collections::BTreeMap<crate::survey::ItemKey, u32>>,
) -> bool {
    let Some(survey) = job.survey.as_ref() else {
        return false;
    };
    // Work nothing holds the blocks for cannot go first: with the rest of a
    // build waiting on a chest, the windows are all there is to do.
    let stocked = |missing: &[ItemStackData]| {
        let Some(held) = held else {
            return true;
        };
        missing.iter().all(|stack| {
            held.get(&crate::survey::key_of(stack))
                .copied()
                .unwrap_or(0)
                >= u32::from(stack.count)
        })
    };
    survey
        .known
        .iter()
        .enumerate()
        .skip(job.crew.pace.cursor)
        .filter(|(_, k)| matches!(k, Known::Place(_) | Known::Clear { .. }))
        .all(|(i, k)| {
            job.crew.built.contains(&i)
                || job.design.glazing(i)
                || matches!(k, Known::Place(missing) if !stocked(missing))
        })
}
