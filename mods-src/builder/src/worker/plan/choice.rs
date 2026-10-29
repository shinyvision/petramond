use crate::geometry::{feet_of, reaches};
use crate::host::prelude::*;
use crate::survey::{key_of, Known};
use crate::worker::stance::Search;
use crate::worker::tuning::hands::{AIM_SETTLE_TICKS, AIM_TICKS, LID_UP, LINGER, SWING};
use crate::worker::tuning::price::{HAND_SECONDS, MOVE_TICKS};
use crate::worker::tuning::window::WEIGHED;
use crate::worker::{cargo, route, scaffold, sight, Body, Ctx, Job, Task};

use super::{clear_of_body, places, supply, task_cells, Flow, Round};

struct Cost {
    ticks: u64,
    units: u64,
}

impl Cost {
    fn cheaper_than(&self, other: &Cost) -> bool {
        self.units > 0 && (other.units == 0 || self.ticks * other.units < other.ticks * self.units)
    }
}

pub(super) fn collects(job: &Job, task: Task) -> bool {
    match task {
        Task::Unit(i) => matches!(
            job.survey.as_ref().map(|s| &s.known[i]),
            Some(Known::Clear { .. })
        ),
        Task::Scaffold(_) | Task::Reopen(_) | Task::Breakout(_) => true,
        Task::Trim(_) | Task::Support { .. } => false,
    }
}

pub(super) fn before_work(
    ctx: &mut Ctx,
    job: &mut Job,
    body: &Body,
    round: &Round,
    task: Task,
    to: [i32; 3],
    setup: i32,
) -> Option<Flow> {
    if !round.supplies.fetch() && !round.supplies.unload {
        return None;
    }
    // A ready placement has no travel to amortize; a supply detour cannot improve its rate.
    if setup == 0 && (places(job, task) || matches!(task, Task::Support { .. })) {
        return None;
    }
    let current = batch(ctx, job, body, round, to, setup.max(0) as u64);
    let trip = match cargo::preview(ctx, job, body, &round.project, !round.supplies.fetch()) {
        Search::Found(trip) => trip,
        Search::Busy | Search::None | Search::Unseen => return None,
    };
    let there = Body {
        slots: trip.slots.clone(),
        ..*body
    };
    let out = MOVE_TICKS * route::moves_or_guess(ctx, body.cell, trip.to);
    let back = MOVE_TICKS * route::moves_or_guess(ctx, trip.to, body.cell);
    let detour = (out + back).max(0) as u64 + LID_UP + LINGER;
    let supplied = batch(ctx, job, &there, round, to, detour + setup.max(0) as u64);
    let here = batch(ctx, job, &there, round, body.cell, detour);
    let already_here = batch(ctx, job, body, round, body.cell, 0);
    if supplied.cheaper_than(&current)
        || (here.cheaper_than(&current)
            && (here.units > already_here.units || here.cheaper_than(&already_here)))
    {
        return Some(supply::choose(ctx, job, round, trip));
    }
    None
}

fn batch(ctx: &mut Ctx, job: &Job, body: &Body, round: &Round, to: [i32; 3], setup: u64) -> Cost {
    let mut result = Cost {
        ticks: setup,
        units: 0,
    };
    let mut held = cargo::totals(&body.slots);
    let full = body.slots.iter().all(Option::is_some);
    let mut seen = Vec::new();
    for (task, _) in round.candidates.iter().chain(&round.future) {
        if seen.contains(task) || (full && collects(job, *task)) {
            continue;
        }
        seen.push(*task);
        if result.units == WEIGHED as u64 {
            break;
        }
        let cells = task_cells(job, *task);
        if !reaches(feet_of(to), &cells) || !clear_of_body(to, &cells) {
            continue;
        }
        let support;
        let missing = match task {
            Task::Unit(i) => match job.survey.as_ref().map(|s| &s.known[*i]) {
                Some(Known::Place(missing)) => missing.as_slice(),
                Some(Known::Clear { .. }) => &[],
                _ => continue,
            },
            Task::Support { .. } => {
                let Some(kind) = scaffold::pick(ctx, job, body) else {
                    continue;
                };
                support = [ItemStackData {
                    item: kind.item.clone(),
                    count: 1,
                    data: Vec::new(),
                }];
                &support
            }
            _ => &[],
        };
        if !cargo::holds(&held, missing) {
            continue;
        }
        if sight::sees(body, &[to], &cells, &sight::work(job, *task))[0].is_some() {
            continue;
        }
        let ticks = if missing.is_empty() {
            let Some(block) = get_block(cells[0]) else {
                continue;
            };
            let Some(info) = ctx.caches.block(block) else {
                continue;
            };
            let hardness = info.hardness.max(0.0);
            let tool = cargo::tool_slot(ctx, &body.slots, block);
            let speed = tool
                .and_then(|slot| body.slots[slot as usize].as_ref())
                .and_then(|stack| ctx.caches.item(&stack.item))
                .and_then(|item| item.tool.as_ref())
                .map_or(1.0, |tool| tool.speed.max(1.0));
            (hardness * HAND_SECONDS / speed / TICK_SECONDS).ceil() as u64
        } else {
            for stack in missing {
                *held.get_mut(&key_of(stack)).unwrap() -= u32::from(stack.count);
            }
            SWING
        };
        result.ticks +=
            ticks.max(SWING) + AIM_SETTLE_TICKS + (AIM_TICKS.start() + AIM_TICKS.end()) / 2;
        result.units += 1;
    }
    result
}
