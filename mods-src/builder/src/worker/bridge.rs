use crate::host::prelude::*;

use super::hands;
use super::tuning::patience::{WALKWAY_PATIENCE, WALKWAY_STEP_UNTAKEN, WALKWAY_SUPPORT_UNLANDED};
use super::{open_block, Body, Ctx, Step, Task};
use crate::content::GOLEM;
use crate::geometry::{feet_of, manhattan, offset, reaches};
use crate::project::Projects;
use crate::worker::Job;

#[derive(Clone, Debug, PartialEq)]
pub struct Bridge {
    pub top: [i32; 3],
    pub path: Vec<[i32; 3]>,
    pub task: Task,
}

impl Bridge {
    fn supports(&self) -> impl Iterator<Item = [i32; 3]> + '_ {
        self.path.iter().map(|p| offset(*p, [0, -1, 0]))
    }
}

pub fn plan(
    ctx: &mut Ctx,
    job: &Job,
    body: &Body,
    task: Task,
    cells: &[[i32; 3]],
    length: i32,
    laid: Option<&Bridge>,
) -> Option<Vec<[i32; 3]>> {
    let top = body.cell;
    let governed = &job.design.governed;
    let mut ends = Vec::new();
    for dx in -length..=length {
        for dz in -length..=length {
            let end = offset(top, [dx, 0, dz]);
            if end != top
                && manhattan(end, top) <= length
                && reaches(feet_of(end), cells)
                && !job.crew.deferrals.blind(task, end)
            {
                ends.push(end);
            }
        }
    }
    ends.sort_by_key(|e| manhattan(*e, top));
    let mut checked = 0;
    for end in ends {
        for path in [l_path(top, end, true), l_path(top, end, false)] {
            if path.iter().any(|p| {
                laid.is_some_and(|w| w.top == *p || w.path.contains(p))
                    || cells.contains(p)
                    || cells.contains(&offset(*p, [0, 1, 0]))
                    || governed.contains(p)
                    || governed.contains(&offset(*p, [0, 1, 0]))
            }) {
                continue;
            }
            let floors: Vec<[i32; 3]> = path.iter().map(|p| offset(*p, [0, -1, 0])).collect();
            let floor_blocks = get_blocks(floors.clone());
            if floors
                .iter()
                .zip(&floor_blocks)
                .any(|(f, b)| governed.contains(f) && !b.is_some_and(|b| !open_block(ctx, b)))
            {
                continue;
            }
            checked += 1;
            if checked > 24 {
                return None;
            }
            let open_floors: Vec<[i32; 3]> = floors
                .iter()
                .zip(&floor_blocks)
                .filter(|(_, b)| b.is_some_and(|b| open_block(ctx, b)))
                .map(|(f, _)| *f)
                .collect();
            if !open_floors.is_empty() && footholds(GOLEM, open_floors).into_iter().any(|ok| ok) {
                continue;
            }
            let body_cells: Vec<[i32; 3]> = path
                .iter()
                .flat_map(|p| [*p, offset(*p, [0, 1, 0])])
                .collect();
            let blocks = get_blocks(body_cells);
            if blocks
                .iter()
                .any(|b| !b.is_some_and(|b| open_block(ctx, b)))
            {
                continue;
            }
            let sees = super::sight::sees(body, &[end], cells, &super::sight::work(job, task));
            if sees.first().is_some_and(Option::is_none) {
                return Some(path);
            }
        }
    }
    None
}

fn l_path(from: [i32; 3], to: [i32; 3], x_first: bool) -> Vec<[i32; 3]> {
    let mut path = Vec::new();
    let mut at = from;
    let mut step = |axis: usize, at: &mut [i32; 3]| {
        while at[axis] != to[axis] {
            at[axis] += (to[axis] - at[axis]).signum();
            path.push(*at);
        }
    };
    if x_first {
        step(0, &mut at);
        step(2, &mut at);
    } else {
        step(2, &mut at);
        step(0, &mut at);
    }
    path
}

/// Walkway goes out a cell at a time. Drop a support, step on it, repeat.
pub fn extend(
    ctx: &mut Ctx,
    projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    since: u64,
    asked: u64,
) -> Step {
    let id = body.id;
    let Some(bridge) = job.crew.aloft.bridge.clone() else {
        return Step::Plan;
    };
    if !body.on_ground {
        return Step::Bridge { since, asked };
    }
    let at = bridge.path.iter().position(|p| *p == body.cell);

    let asked = match at {
        Some(i)
            if asked != 0
                && get_block(offset(bridge.path[i], [0, -1, 0]))
                    .is_some_and(|b| !open_block(ctx, b))
                && job.crew.presence.goal.is_none_or(|goal| goal == body.cell) =>
        {
            0
        }
        _ => asked,
    };
    let next = match at {
        Some(i) if i + 1 == bridge.path.len() => {
            job.crew.presence.set_goal(id, None);
            return Step::Plan;
        }
        Some(i) => bridge.path[i + 1],
        None if body.cell == bridge.top => bridge.path[0],
        None => return give_up(ctx, job, &bridge),
    };
    if ctx.now > since + WALKWAY_PATIENCE {
        return give_up(ctx, job, &bridge);
    }
    let support = offset(next, [0, -1, 0]);
    if get_block(support).is_some_and(|b| !open_block(ctx, b)) {
        if asked == 0 {
            match super::route::probe(ctx, body.cell, next, Vec::new()) {
                Some(Route::Open) => {}
                Some(_) => {
                    trace!("TRACE bridge step {next:?} not walkable");
                    return give_up(ctx, job, &bridge);
                }
                None => return Step::Bridge { since, asked },
            }
        } else if ctx.now > asked + WALKWAY_STEP_UNTAKEN {
            trace!("TRACE bridge step {next:?} never taken");
            return give_up(ctx, job, &bridge);
        }
        job.crew.presence.face(id, None);
        job.crew.presence.set_hold(id, false);
        job.crew.presence.set_goal(id, Some(next));
        return Step::Bridge {
            since,
            asked: if asked == 0 { ctx.now } else { asked },
        };
    }
    if asked != 0 && ctx.now > asked + WALKWAY_SUPPORT_UNLANDED {
        trace!(
            "TRACE bridge support {support:?} never landed: at {at:?} cell {:?} pos {:?} goal {:?} asked {asked} now {}",
            body.cell, body.pos, job.crew.presence.goal, ctx.now
        );
        return give_up(ctx, job, &bridge);
    }
    if asked != 0 {
        return Step::Bridge { since, asked };
    }
    job.crew.presence.set_goal(id, None);
    job.crew.presence.set_hold(id, true);
    let work = super::sight::scaffold(job, support);
    let unseen = super::sight::sees_from(body, vec![body.pos], &[support], &work)[0].is_some();
    if unseen && super::sight::still_turning(job) {
        super::lean(body, support);
        return Step::Bridge { since, asked: 0 };
    }
    match super::scaffold::lay(ctx, job, body, support) {
        hands::Lay::Turning => Step::Bridge { since, asked: 0 },
        hands::Lay::Queued => {
            projects.update(job.id, |p| {
                if !p.scaffolds.contains(&support) {
                    p.scaffolds.push(support);
                }
            });
            Step::Bridge {
                since,
                asked: ctx.now,
            }
        }
        hands::Lay::Satisfied => Step::Bridge { since, asked: 0 },
        hands::Lay::Refused(refusal) => {
            trace!("TRACE bridge support {support:?} refused {refusal:?}");
            give_up(ctx, job, &bridge)
        }
    }
}

fn give_up(ctx: &Ctx, job: &mut Job, bridge: &Bridge) -> Step {
    if let Task::Unit(i) = bridge.task {
        job.crew.deferrals.tried.insert(i);
    }
    Step::Unbridge { since: ctx.now }
}

pub fn retract(
    ctx: &mut Ctx,
    projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    since: u64,
) -> Step {
    let id = body.id;
    let Some(bridge) = job.crew.aloft.bridge.clone() else {
        return Step::Plan;
    };
    if !body.on_ground {
        return Step::Unbridge { since };
    }
    let Some(project) = projects.get(job.id).cloned() else {
        return Step::Plan;
    };
    let content = ctx.content;
    let ours = |cell: [i32; 3]| {
        project.scaffolds.contains(&cell) && super::scaffold::stands(content, cell) == Some(true)
    };
    let at = bridge.path.iter().position(|p| *p == body.cell);
    let under = offset(body.cell, [0, -1, 0]);
    let farther: Vec<[i32; 3]> = match at {
        Some(i) => bridge.supports().skip(i + 1).collect(),
        None => bridge.supports().collect(),
    };
    let farther: Vec<[i32; 3]> = farther
        .into_iter()
        .filter(|s| *s != under && ours(*s))
        .collect();
    if farther.last().is_some_and(|s| !reaches(body.pos, &[*s])) {
        let next = match at {
            Some(i) => bridge.path.get(i + 1),
            None if body.cell == bridge.top => bridge.path.first(),
            None => None,
        };
        if let Some(next) = next.filter(|n| ours(offset(**n, [0, -1, 0]))) {
            job.crew.presence.face(id, None);
            job.crew.presence.set_hold(id, false);
            job.crew.presence.set_goal(id, Some(*next));
            return Step::Unbridge { since };
        }
    }
    if let Some(support) = farther.last().copied() {
        if reaches(body.pos, &[support]) {
            job.crew.presence.set_goal(id, None);
            job.crew.presence.set_hold(id, true);
            match hands::dig(ctx, job, body, support, true) {
                hands::Dig::Turning | hands::Dig::Digging | hands::Dig::Breaking => {
                    return Step::Unbridge { since }
                }
                hands::Dig::Nothing => {
                    projects.update(job.id, |p| p.scaffolds.retain(|c| *c != support));
                }
                hands::Dig::Refused(_) => {}
            }
        }
    }
    if at.is_none() || ctx.now > since + WALKWAY_PATIENCE {
        for left in bridge.supports().filter(|s| ours(*s)) {
            if !job.crew.scaffolding.urgent.contains(&left) {
                job.crew.scaffolding.urgent.push(left);
            }
        }
        job.crew.aloft.bridge = None;
        job.crew.presence.set_goal(id, None);
        return Step::Plan;
    }
    let back = match at {
        Some(0) | None => bridge.top,
        Some(i) => bridge.path[i - 1],
    };
    job.crew.presence.face(id, None);
    job.crew.presence.set_hold(id, false);
    job.crew.presence.set_goal(id, Some(back));
    Step::Unbridge { since }
}
