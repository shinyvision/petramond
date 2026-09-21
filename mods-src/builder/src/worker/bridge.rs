//! Branching scaffolding: from a pillar top the golem lays a walkway of
//! scaffolds toward work no perch reaches (the middle of a roof), works from
//! its end, and takes it down on the way back.
//!
//! Each support goes one level under the next path cell, so the one before
//! gives it a face and it stays in sight; walking back, each support is dug as
//! soon as it is stepped off, while still in sight.

use mod_sdk::*;

use super::hands;
use super::tuning::patience::{WALKWAY_PATIENCE, WALKWAY_STEP_UNTAKEN, WALKWAY_SUPPORT_UNLANDED};
use super::{open_block, Body, Ctx, Step, Task};
use crate::content::GOLEM;
use crate::geometry::{feet_of, manhattan, offset, reaches};
use crate::jobs::Job;
use crate::project::Projects;

#[derive(Clone, Debug, PartialEq)]
pub struct Bridge {
    /// Where the golem stood on the pillar top.
    pub top: [i32; 3],
    /// The feet cells walked, top excluded, in order out.
    pub path: Vec<[i32; 3]>,
    /// The work the walkway was laid for, done first at its end.
    pub task: Task,
}

impl Bridge {
    fn supports(&self) -> impl Iterator<Item = [i32; 3]> + '_ {
        self.path.iter().map(|p| offset(*p, [0, -1, 0]))
    }
}

/// The cells of a walkway no longer than `length` from where the golem stands
/// out to standing room that sees `cells`, shortest first, crossing none of
/// the `laid` walkway it goes on from.
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
            // Built design (a roof course's stairs) is floor enough; unbuilt
            // design must not get a scaffold in its place.
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
            // Ground a step below the walkway is walked, not bridged: a body
            // steps down onto it off the walkway and leaves it standing.
            let open_floors: Vec<[i32; 3]> = floors
                .iter()
                .zip(&floor_blocks)
                .filter(|(_, b)| b.is_some_and(|b| open_block(ctx, b)))
                .map(|(f, _)| *f)
                .collect();
            if !open_floors.is_empty() && footholds(GOLEM, open_floors).into_iter().any(|ok| ok) {
                continue;
            }
            // Room for the body along the way; supports either to lay or
            // already standing (a roof course walked over).
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

/// Lay the walkway out: a support under the next cell, then the step onto it.
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

    // Each walkway cell restarts the clock, walked onto or not: leaning out to
    // lay a support puts the body on the new cell as it lands.
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
        // A step the body cannot take (a slab lip, an eave over the head)
        // ends this walkway; so does one not taken within a few seconds.
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
    // A placement answers within a tick or two; one that never shows was
    // refused when its turn came.
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
    // The face a walkway goes on against points away from a body over the
    // middle of its block: it leans out over the edge until it sees it.
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

/// Take a walkway that cannot be laid back down, and do not lay it again for
/// the same work from this pillar.
fn give_up(ctx: &Ctx, job: &mut Job, bridge: &Bridge) -> Step {
    if let Task::Unit(i) = bridge.task {
        job.crew.deferrals.tried.insert(i);
    }
    Step::Unbridge { since: ctx.now }
}

/// Walk the walkway back to the pillar top, digging each support just left.
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
    // Walked back along it (to work from partway): out again first while the
    // far end stands out of reach, or it is left hanging there.
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
    // Farthest first: dug nearest first, a hole opens between the golem and
    // what is left out there.
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
        // Back on the top (or given up, or stepped off): what is left is
        // ordinary scaffolding, taken down first.
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
    // A gaze left on the last block laid holds the body turned toward it.
    job.crew.presence.face(id, None);
    job.crew.presence.set_hold(id, false);
    job.crew.presence.set_goal(id, Some(back));
    Step::Unbridge { since }
}
