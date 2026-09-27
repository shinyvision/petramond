use super::climb::climb_toward;
use super::sealing::{cutting, strands};
use super::verdict::{settle_verdict, viability, waits_there, Viable};
use super::{defer_task, places, task_cells, walk_via, Flow, Round};
use crate::geometry::{feet_of, manhattan, offset, reaches};
use crate::project::Projects;
use crate::worker::route::{self, Hubs};
use crate::worker::tuning::patience::NOWHERE_STANDS;
use crate::worker::tuning::price::{LEVEL_MOVES, LEVEL_TICKS, MOUNT_TICKS, MOVE_TICKS};
use crate::worker::tuning::reach::PILLAR_SPAN;
use crate::worker::tuning::waits::{SEALED_WAIT, STRANDING_STANCE};
use crate::worker::tuning::window::{FOCUS_REACH, SEARCHES, SPOTS, STANCE_NEARBY, STANCE_SEARCHES};
use crate::worker::waiting::{Probe, Waiting};
use crate::worker::Job;
use crate::worker::{pillar, sight, stance, Body, Ctx, Task, Then};

pub(super) struct Spot {
    task: Task,
    leg: [i32; 3],
    to: [i32; 3],
    ticks: i32,
}

pub(super) fn walk_costs(
    ctx: &mut Ctx,
    job: &Job,
    body: &Body,
    cells: &[(Task, [i32; 3])],
) -> Option<Vec<Option<u32>>> {
    let region = route::region(ctx, body.cell, false, &[])??;
    Some(
        cells
            .iter()
            .map(|(_, cell)| {
                let mut best: Option<u32> = None;
                for dy in -5..=2 {
                    for dx in -4..=4 {
                        for dz in -4..=4 {
                            let stance = offset(*cell, [dx, dy, dz]);
                            let Some(moves) = region.moves(stance) else {
                                continue;
                            };
                            if best.is_none_or(|b| moves < b) && reaches(feet_of(stance), &[*cell])
                            {
                                best = Some(moves);
                            }
                        }
                    }
                }
                if best.is_none() {
                    let inside = job.design.over_floor(*cell);
                    for dy in -PILLAR_SPAN..-5 {
                        for dx in -3..=3 {
                            for dz in -3..=3 {
                                let foot = offset(*cell, [dx, dy, dz]);
                                if job.design.over_floor(foot) != inside {
                                    continue;
                                }
                                if let Some(moves) = region.moves(foot) {
                                    let cost = moves + LEVEL_MOVES as u32 * dy.unsigned_abs();
                                    if best.is_none_or(|b| cost < b) {
                                        best = Some(cost);
                                    }
                                }
                            }
                        }
                    }
                }
                best
            })
            .collect(),
    )
}

pub(super) fn rank_by_walk(
    ctx: &mut Ctx,
    _projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    round: &mut Round,
) -> Flow {
    let mut candidates = std::mem::take(&mut round.candidates);
    let focus = job.crew.pace.focus.filter(|f| {
        candidates
            .iter()
            .any(|(_, c)| manhattan(*c, *f) <= FOCUS_REACH)
    });
    job.crew.pace.focus = focus;
    let mut walks: Vec<Option<u32>> = vec![None; candidates.len()];
    if let Some(costs) = walk_costs(ctx, job, body, &candidates) {
        type Order = (bool, bool, bool, u32, i32);
        let mut keyed: Vec<((Task, [i32; 3]), Order)> = candidates
            .into_iter()
            .zip(costs)
            .map(|((task, c), cost)| {
                let urgent = task.urgent(&job.crew.scaffolding.urgent);
                let late = task.late(&job.design);
                let afield = focus.is_some_and(|f| manhattan(c, f) > FOCUS_REACH);
                let key = (
                    !urgent,
                    late,
                    afield,
                    cost.unwrap_or(u32::MAX),
                    manhattan(c, body.cell),
                );
                ((task, c), key)
            })
            .collect();
        keyed.sort_by_key(|(_, key)| *key);
        walks = keyed
            .iter()
            .map(|(_, key)| Some(key.3).filter(|cost| *cost != u32::MAX))
            .collect();
        candidates = keyed.into_iter().map(|(candidate, _)| candidate).collect();
    }
    if let Some((_, c)) = candidates.first() {
        job.crew.pace.focus.get_or_insert(*c);
    }
    round.candidates = candidates;
    round.walks = walks;
    Flow::Pass
}

pub(super) fn stance_spots(
    ctx: &mut Ctx,
    _projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    round: &mut Round,
) -> Flow {
    let project = &round.project;
    let candidates = &round.candidates;
    let walks = &round.walks;
    let nearest = walks.first().copied().flatten();
    let mut wants_perch: Vec<(Task, bool)> = Vec::new();
    let mut spots: Vec<Spot> = Vec::new();
    for (n, (task, _)) in candidates.iter().enumerate().take(STANCE_SEARCHES) {
        let nearby = match (nearest, walks[n]) {
            (Some(nearest), Some(walk)) => walk <= nearest + STANCE_NEARBY,
            _ => n < SEARCHES,
        };
        if !nearby && n >= SEARCHES {
            break;
        }
        if job.crew.deferrals.deferred(*task, ctx.now) {
            continue;
        }
        let verdict = viability(ctx, job, body, *task, body.pos);
        if !matches!(verdict, Viable::Now | Viable::Elsewhere) {
            settle_verdict(ctx, job, *task, verdict);
            continue;
        }
        if let Some(unseen) = job.crew.deferrals.still_nowhere(*task, body.cell, ctx.now) {
            if wants_perch.len() < SEARCHES {
                wants_perch.push((*task, unseen));
            }
            continue;
        }
        let cells = task_cells(job, *task);
        let digging = !places(job, *task);
        let trail = job.crew.trail.clone();
        let hubs = Hubs::new(project.home, &trail);
        let crew = &job.crew;
        let filled = &job.design.filled;
        let usable = |s: [i32; 3]| {
            !crew.deferrals.blind(*task, s)
                && !filled.contains(&s)
                && !filled.contains(&offset(s, [0, 1, 0]))
        };
        let mut unseen = false;
        let work = sight::work(job, *task);
        let mut found = stance::find(ctx, body, hubs, &cells, &work, digging, usable);
        if matches!(found, stance::Search::None | stance::Search::Unseen) {
            let relaxed = stance::find(ctx, body, hubs, &cells, &work, digging, |s| {
                !crew.deferrals.blind(*task, s)
                    && (filled.contains(&s) || filled.contains(&offset(s, [0, 1, 0])))
            });
            if !matches!(relaxed, stance::Search::None) {
                found = relaxed;
            }
        }
        match found {
            stance::Search::Found(to) => {
                if waits_there(ctx, job, body, *task, to) {
                    continue;
                }
                if matches!(task, Task::Support { .. })
                    || (matches!(task, Task::Unit(_)) && !digging)
                {
                    match strands(ctx, job, project, to, *task, &cells) {
                        Some(true) => {
                            job.crew.deferrals.strike(*task, to);
                            defer_task(ctx, job, *task, STRANDING_STANCE);
                            continue;
                        }
                        None => {
                            return Flow::Busy(Waiting::Probe(Probe::StanceSealing));
                        }
                        Some(false) => {}
                    }
                    match cutting(ctx, job, project, *task, &cells) {
                        Some(true) => {
                            defer_task(ctx, job, *task, SEALED_WAIT);
                            continue;
                        }
                        None => {
                            return Flow::Busy(Waiting::Probe(Probe::StanceSealing));
                        }
                        Some(false) => {}
                    }
                }
                match route::leg(ctx, hubs, body, to) {
                    Some(Some(cell)) => {
                        let moves = route::moves_or_guess(ctx, body.cell, to);
                        if !spots.iter().any(|spot| spot.to == to) {
                            spots.push(Spot {
                                task: *task,
                                leg: cell,
                                to,
                                ticks: MOVE_TICKS * moves.max(1),
                            });
                        }
                        if spots.len() == SPOTS {
                            break;
                        }
                        continue;
                    }
                    Some(None) => {
                        job.crew.deferrals.strike(*task, to);
                        continue;
                    }
                    None => {
                        return Flow::Busy(Waiting::Probe(Probe::Walk));
                    }
                }
            }
            stance::Search::Busy => {
                return Flow::Busy(Waiting::Probe(Probe::Stance));
            }
            stance::Search::None => {}
            stance::Search::Unseen => unseen = true,
        }
        job.crew
            .deferrals
            .found_nowhere(*task, body.cell, ctx.now + NOWHERE_STANDS, unseen);
        if wants_perch.len() < SEARCHES {
            wants_perch.push((*task, unseen));
        }
    }
    round.wants_perch = wants_perch;
    round.spots = spots;
    Flow::Pass
}

pub(super) fn nearest_spot(
    ctx: &mut Ctx,
    _projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    round: &mut Round,
) -> Flow {
    let project = &round.project;
    let candidates = &round.candidates;
    let (spots, wants_perch) = (&round.spots, &round.wants_perch);
    if !spots.is_empty() {
        let open: Vec<[i32; 3]> = candidates.iter().map(|(_, c)| *c).collect();
        let worth = |ticks: i32, lays: i32| ticks * 16 / lays.max(1);
        let mut best: Option<(usize, i32)> = None;
        for (n, spot) in spots.iter().enumerate() {
            let lays = pillar::lays(job, body, &[spot.to], &open, spot.to)[0];
            let worth = worth(spot.ticks, lays);
            if best.is_none_or(|(_, least)| worth < least) {
                best = Some((n, worth));
            }
        }
        let (n, least) = best.expect("spots is not empty");
        if let Some((task, _)) = wants_perch.first().copied() {
            let cells = task_cells(job, task);
            match pillar::find(ctx, job, project, body, task, &cells, &open) {
                stance::Search::Found(found) => {
                    let top = found.top_cell();
                    let foot = found.foot_cell();
                    let moves = route::moves_or_guess(ctx, body.cell, foot);
                    let ticks =
                        MOVE_TICKS * moves + LEVEL_TICKS * (found.top - found.base) + MOUNT_TICKS;
                    let lays = pillar::lays(job, body, &[top], &open, top)[0];
                    if worth(ticks, lays) < least {
                        let trail = job.crew.trail.clone();
                        let hubs = Hubs::new(project.home, &trail);
                        if let Some(flow) =
                            climb_toward(ctx, job, project, hubs, body, task, &cells, found)
                        {
                            return flow;
                        }
                    }
                }
                stance::Search::Busy => {
                    return Flow::Busy(Waiting::Probe(Probe::Pillar));
                }
                stance::Search::None | stance::Search::Unseen => {}
            }
        }
        let spot = &spots[n];
        return Flow::Go(walk_via(ctx, spot.leg, spot.to, Then::Task(spot.task)));
    }
    Flow::Pass
}
