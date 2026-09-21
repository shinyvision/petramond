//! A walk toward a goal, a leg at a time, and giving it up.

use super::tuning::body::CHEST_OFF_CENTRE;
use super::tuning::patience::{
    ASTRAY_MOVES, CHEST_CENTRE_TICKS, OFF_ROUTE_TICKS, WALK_STALL, WALK_STALLS,
};
use super::tuning::waits::WALK_FAILED;
use super::{arrive, centre, rescue, route, Body, Ctx, Step, Task, Then};
use crate::jobs::Job;
use crate::project::Projects;

#[allow(clippy::too_many_arguments)]
pub(super) fn walk(
    ctx: &mut Ctx,
    projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    to: [i32; 3],
    goal: [i32; 3],
    then: Then,
    best: i32,
    progress: u64,
) -> Step {
    job.crew.presence.set_hold(body.id, false);
    job.crew.presence.set_goal(body.id, Some(to));
    job.crew.presence.face(body.id, None);
    job.crew.presence.animate(body.id, None);
    // On the way to lay a block, it is already in hand.
    if let Then::Task(Task::Unit(i)) = then {
        if let Some(crate::survey::Known::Place(missing)) = job.survey.as_ref().map(|s| &s.known[i])
        {
            let item = missing.first().map(|s| s.item.clone());
            job.crew.presence.hold_item(body.id, item);
        }
    }
    // A leg walked: the next one, judged from here, until the goal.
    if body.cell == to && body.on_ground && to != goal {
        let Some(home) = projects.get(job.id).map(|p| p.home) else {
            return Step::Plan;
        };
        let trail = job.crew.trail.clone();
        let hubs = route::Hubs::new(home, &trail);
        return match route::leg(ctx, hubs, body, goal) {
            Some(Some(next)) => Step::Walk {
                to: next,
                goal,
                then,
                best: i32::MAX,
                progress: ctx.now,
            },
            Some(None) => Step::Plan,
            None => Step::Walk {
                to,
                goal,
                then,
                best,
                progress,
            },
        };
    }
    if body.cell == to && body.on_ground {
        job.crew.presence.set_goal(body.id, None);
        // Reach to a chest is judged from the cell's centre; on the cell's
        // edge it may fall short and the fetch find nothing to take.
        let [dx, dz] = body.off_centre();
        if matches!(then, Then::Fetch(_) | Then::Deposit(_))
            && dx.abs().max(dz.abs()) > CHEST_OFF_CENTRE
            && ctx.now <= progress + CHEST_CENTRE_TICKS
        {
            centre(body);
            return Step::Walk {
                to,
                goal,
                then,
                best,
                progress,
            };
        }
        return arrive(ctx, projects, job, body, then);
    }
    // Progress is judged along the way, not as the crow flies (the way to an
    // upper floor goes round by the door): the flood back from the goal ranks
    // footholds by moves. Off it, or out of the site, the walk is over.
    let (min, max) = ctx.site;
    let off_site = (0..3).any(|i| body.cell[i] < min[i] || body.cell[i] > max[i]);
    let (distance, off_route) = match route::region(ctx, goal, true, &[]) {
        Some(Some(toward)) => match toward.moves(body.cell) {
            // Moves from the goal only fall along a way to it: a body well
            // farther off than its best is being led astray, and as lost as one
            // off the flood.
            Some(moves) => (
                moves as i32,
                best != i32::MAX && moves as i32 > best + ASTRAY_MOVES,
            ),
            None => (best, body.on_ground && best != i32::MAX),
        },
        Some(None) => (crate::geometry::manhattan(body.cell, goal), false),
        None => (best, false),
    };
    // A body mid-step on a stair stands a moment in a cell the flood does not
    // list: only a while off it is lost.
    if !off_route {
        job.crew.rescue.off_route_since = None;
    }
    let since = *job.crew.rescue.off_route_since.get_or_insert(ctx.now);
    let lost = off_route && ctx.now >= since + OFF_ROUTE_TICKS;
    if best != i32::MAX && (lost || off_site) {
        give_up(ctx, job, body, goal, then);
        return Step::Plan;
    }
    if distance < best {
        return Step::Walk {
            to,
            goal,
            then,
            best: distance,
            progress: ctx.now,
        };
    }
    if ctx.now > progress + WALK_STALL {
        give_up(ctx, job, body, goal, then);
        // Near the same spot counts as the same stall: a body bobbing against
        // a step or a ceiling changes its cell every failed walk.
        let stalled = &mut job.crew.rescue.stalled;
        *stalled = (
            body.cell,
            if crate::geometry::manhattan(stalled.0, body.cell) <= 2 {
                stalled.1 + 1
            } else {
                1
            },
        );
        // Walks that keep failing where a route says the way is open are no
        // state to get out of by walking: the golem comes up at home. Where
        // no route leads home, the next plan finds its way out.
        if stalled.1 >= WALK_STALLS {
            job.crew.rescue.stalled.1 = 0;
            if let Some(home) = projects.get(job.id).map(|p| p.home) {
                let trail = job.crew.trail.clone();
                let hubs = route::Hubs::new(home, &trail);
                if job.crew.aloft.perch.is_some() || !rescue::stranded(ctx, hubs, body) {
                    trace!(
                        "TRACE walks keep failing at {:?}: coming up at home",
                        body.cell
                    );
                    return rescue::relocate(job);
                }
            }
        }
        return Step::Plan;
    }
    Step::Walk {
        to,
        goal,
        then,
        best,
        progress,
    }
}

/// The walk is over short of its goal. The stance that could not be walked
/// to is struck for the task, as one it was refused from is: planned again,
/// it fails again.
pub(super) fn give_up(ctx: &Ctx, job: &mut Job, body: &Body, goal: [i32; 3], then: Then) {
    job.crew.presence.set_goal(body.id, None);
    if let Then::Task(task) | Then::Course(task) = then {
        job.crew.deferrals.defer(task, ctx.now + WALK_FAILED);
        job.crew.deferrals.strike(task, goal);
    }
    if let (Then::Escape, Some(stuck)) = (then, job.crew.rescue.stuck.as_mut()) {
        stuck.walk_failed(body.cell, goal);
    }
    job.crew.note = "The golem could not walk where it needed to".into();
}
