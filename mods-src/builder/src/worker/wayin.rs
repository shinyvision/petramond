//! Getting to work the world stands in the way of: a door shut across the
//! route, ground the design says nothing about, a threshold the golem is
//! standing on. Everything here is about REACHING work, never about escaping.

use crate::fx::{HashMap, HashSet};

use mod_sdk::*;

use super::ground::{ground, search, Move};
use super::plan::walk_to;
use super::route::{self, Hubs};
use super::tuning::{
    every::{DIG_SCAN_EVERY, DOOR_SCAN_EVERY},
    patience::{USE_PATIENCE, WAY_IN_PATIENCE},
    reach::{
        DIG_ABOVE, DIG_AROUND, DIG_BELOW, DOOR_PROBES, DOOR_REACH, NO_GO, SCAN_RESERVE, SHUT_OUT,
    },
    waits::{DIG_REST, DOOR_AGAIN, DOOR_REST},
};
use super::{Body, Ctx, Step, Task, Then};
use crate::design::paged;
use crate::geometry::{feet_of, manhattan, offset, SIDES};
use crate::jobs::Job;
use crate::project::Project;

/// Whether this tick can afford a scan at all.
fn worth_asking(ctx: &Ctx) -> bool {
    *ctx.probe_nodes > SCAN_RESERVE
}

/// A shut door between the golem and work it cannot otherwise reach: walked
/// to and opened, as a builder would. `None` when no door is in the way.
pub fn door_toward(ctx: &mut Ctx, job: &mut Job, body: &Body, cells: &[[i32; 3]]) -> Option<Step> {
    // Reading every cell beside where the golem walks is dear: it looks for a
    // door in the way now and then, not at every piece of work it cannot reach.
    if ctx.now < job.crew.access.door_scan_at + DOOR_SCAN_EVERY || !worth_asking(ctx) {
        return None;
    }
    job.crew.access.door_scan_at = ctx.now;
    let door = door_in_the_way(ctx, job, body, cells);
    if door.is_none() {
        job.crew.access.door_scan_at = ctx.now + DOOR_REST;
    }
    door
}

fn door_in_the_way(ctx: &mut Ctx, job: &mut Job, body: &Body, cells: &[[i32; 3]]) -> Option<Step> {
    let here: HashMap<[i32; 3], u32> = match route::region(ctx, body.cell, false, &[]) {
        Some(Some(region)) => region
            .cells()
            .filter_map(|c| Some((c, region.moves(c)?)))
            .collect(),
        _ => {
            if super::TRACE {
                log("TRACE door scan: the ground around has not loaded");
            }
            return None;
        }
    };
    // Only work the golem cannot walk to is behind a door.
    if cells.iter().any(|c| here.contains_key(c)) {
        return None;
    }
    // One scan answers for all the work shut out, not just the piece weighed
    // first: the nearest door may be next to another piece.
    let mut shut_out: Vec<usize> = job.crew.access.unreachable.iter().copied().collect();
    shut_out.sort_unstable();
    shut_out.truncate(SHUT_OUT);
    let mut wanted: Vec<[i32; 3]> = cells.to_vec();
    for unit in shut_out {
        wanted.extend(super::plan::task_cells(job, Task::Unit(unit)));
    }
    wanted.sort_unstable();
    wanted.dedup();
    let mut beside: Vec<[i32; 3]> = here
        .keys()
        .filter(|c| wanted.iter().any(|w| manhattan(**c, *w) <= DOOR_REACH))
        .flat_map(|c| {
            let mut cells = vec![*c];
            cells.extend(SIDES.map(|s| offset(*c, s)));
            cells
        })
        .flat_map(|n| [n, offset(n, [0, 1, 0])])
        .collect();
    beside.sort_unstable();
    beside.dedup();
    let blocks = paged(beside.clone(), get_blocks);
    let mut doors: Vec<[i32; 3]> = Vec::new();
    for (cell, block) in beside.iter().zip(blocks) {
        let Some(block) = block else { continue };
        if !ctx
            .caches
            .block(block)
            .is_some_and(|info| info.interaction == Some(BlockUse::ToggleDoor))
        {
            continue;
        }
        let below = offset(*cell, [0, -1, 0]);
        let lower = if get_block(below) == Some(block) {
            below
        } else {
            *cell
        };
        let tried = job
            .crew
            .access
            .door_tried
            .get(&lower)
            .is_some_and(|at| ctx.now < at + DOOR_AGAIN);
        if !doors.contains(&lower) && !tried {
            doors.push(lower);
        }
    }
    // A door with walking room on both sides is already open or no longer the
    // way through: toggling it would only shut it.
    doors.retain(|door| {
        let beside = SIDES.map(|s| offset(*door, s));
        let walked = beside.iter().filter(|n| here.contains_key(*n)).count();
        walked >= 1
            && walked
                < beside
                    .iter()
                    .filter(|n| !job.design.filled.contains(*n))
                    .count()
    });
    // The door nearest the work, opened from where the golem can stand.
    doors.sort_by_key(|d| {
        (
            wanted.iter().map(|w| manhattan(*d, *w)).min().unwrap_or(0),
            *d,
        )
    });
    trace!(
        "TRACE door scan for {:?}: region {} cells, {} beside, doors {doors:?}",
        cells[0],
        here.len(),
        beside.len()
    );
    // A door is only worth touching while it stands in the way; toggling an
    // open one shuts it. Asked as a step ACROSS the door, from the walking
    // side: a probe from wherever the golem stands is a long search, asked
    // every scan.
    let mut blocking = None;
    for shut in doors.into_iter().take(DOOR_PROBES) {
        let mut near = SIDES.iter().map(|s| offset(shut, *s));
        let far: Vec<[i32; 3]> = near.clone().filter(|n| !here.contains_key(n)).collect();
        let Some(step) = near.find(|n| here.contains_key(n)) else {
            continue;
        };
        let mut blocks = false;
        for far in far.into_iter().take(DOOR_PROBES) {
            match route::probe(ctx, step, far, Vec::new()) {
                Some(Route::Open) => {}
                Some(_) => blocks = true,
                None => return None,
            }
        }
        if blocks {
            blocking = Some(shut);
            break;
        }
    }
    let door = blocking?;
    let halves = [door, offset(door, [0, 1, 0])];
    let mut stances: Vec<([i32; 3], u32)> = here
        .iter()
        // Never from inside the doorway itself: a panel swung through the
        // golem is no way through, and the reach at its own cell is edge on.
        .filter(|(s, _)| !halves.contains(s) && !halves.contains(&offset(**s, [0, 1, 0])))
        .filter(|(s, _)| crate::geometry::reaches(feet_of(**s), &halves))
        .map(|(s, m)| (*s, *m))
        .collect();
    stances.sort_by_key(|(s, m)| (*m, *s));
    stances.truncate(16);
    let sees = super::sight::clicks_any(
        body,
        stances.iter().map(|(s, _)| feet_of(*s)).collect(),
        &halves,
    );
    let stance = stances
        .into_iter()
        .zip(sees)
        .find(|(_, refusal)| refusal.is_none())
        .map(|((s, _), _)| s)?;
    job.crew.access.door_tried.insert(door, ctx.now);
    trace!(
        "TRACE opening the door at {door:?} from {stance:?} for work at {:?}",
        cells[0]
    );
    if stance == body.cell {
        return Some(Step::Use {
            door,
            open: true,
            since: ctx.now,
        });
    }
    Some(walk_to(ctx, stance, Then::Open(door)))
}

/// Turn to the door in reach and swing it once the eyes are on it. One
/// opened is shut again on the way home.
pub fn use_door(
    ctx: &mut Ctx,
    job: &mut Job,
    body: &Body,
    door: [i32; 3],
    open: bool,
    since: u64,
) -> Step {
    job.crew.presence.set_goal(body.id, None);
    job.crew.presence.set_hold(body.id, true);
    // Either half: the lower one is often hidden behind the door's own upper
    // half.
    let mut swung = false;
    let mut turning = false;
    for half in [door, offset(door, [0, 1, 0])] {
        match super::hands::use_block(ctx, job, body, half) {
            super::hands::Use::Done => {
                swung = true;
                break;
            }
            super::hands::Use::Turning => {
                turning = true;
                break;
            }
            super::hands::Use::Refused => {}
        }
    }
    if swung {
        // Every way remembered through here was judged as it stood.
        ctx.routes.clear();
        ctx.regions.clear();
    }
    if turning && ctx.now <= since + USE_PATIENCE {
        return Step::Use { door, open, since };
    }
    if open {
        if swung && !job.crew.access.opened.contains(&door) {
            job.crew.access.opened.push(door);
        }
    } else {
        job.crew.access.opened.retain(|d| *d != door);
    }
    job.crew.presence.set_hold(body.id, false);
    Step::Plan
}

/// Getting cells open: moves, the cells dug, and the door opened.
pub fn off_threshold(ctx: &mut Ctx, hubs: Hubs, body: &Body) -> Option<Step> {
    let halves = [body.cell, offset(body.cell, [0, 1, 0])];
    let doors = paged(halves.to_vec(), get_blocks);
    let standing = doors.iter().any(|b| {
        b.and_then(|b| ctx.caches.block(b))
            .is_some_and(|info| info.interaction == Some(BlockUse::ToggleDoor))
    });
    if !standing {
        return None;
    }
    let out: Vec<[i32; 3]> = SIDES.iter().map(|s| offset(body.cell, *s)).collect();
    let room = footholds(crate::content::GOLEM, out.clone());
    for (cell, standing) in out.into_iter().zip(room) {
        if !standing {
            continue;
        }
        // The panel stands across one way out of its own cell: the step off
        // must be one the golem can take, not merely one that walks home from
        // there.
        match route::probe(ctx, body.cell, cell, Vec::new()) {
            Some(Route::Open) => {}
            Some(_) => continue,
            None => return None,
        }
        if matches!(route::out(ctx, hubs, cell, &[]), Some(Route::Open)) {
            trace!("TRACE off the threshold at {:?} to {cell:?}", body.cell);
            return Some(walk_to(ctx, cell, Then::Escape));
        }
    }
    None
}

pub(super) fn open(ctx: &mut Ctx, cell: [i32; 3]) -> bool {
    get_block(cell).is_some_and(|b| super::open_block(ctx, b))
}

/// Dig to work that ground shuts off (a wall against a bank, a room never dug
/// out), weighed like a way out, from reachable standing room to beside the
/// work. Design blocks are priced dear, so it cuts through the hill, not the
/// house.
pub fn dig_toward(
    ctx: &mut Ctx,
    job: &mut Job,
    body: &Body,
    project: &Project,
    cells: &[[i32; 3]],
) -> Option<Step> {
    if ctx.now < job.crew.access.dig_scan_at + DIG_SCAN_EVERY || !worth_asking(ctx) {
        return None;
    }
    job.crew.access.dig_scan_at = ctx.now;
    // A golem already digging its way in keeps the quick cadence: the rest is
    // for the speculative question, asked of work that may not be dug to at all.
    let digging = job.crew.access.way_in.is_some();
    let way = dig_way(ctx, job, body, project, cells);
    if way.is_none() && !digging {
        // Nothing to dig toward: rest before asking again. The question floods
        // the site from the budget the planner finds stances with, and asked
        // too often it starves them.
        job.crew.access.dig_scan_at = ctx.now + DIG_REST;
    }
    way
}

fn dig_way(
    ctx: &mut Ctx,
    job: &mut Job,
    body: &Body,
    project: &Project,
    cells: &[[i32; 3]],
) -> Option<Step> {
    // A way in half dug is worth more than a fresh one somewhere else: the
    // work it was begun for is dug to until it is reached or given up on.
    if let Some((_, since)) = job.crew.access.way_in {
        if ctx.now > since + WAY_IN_PATIENCE {
            job.crew.access.way_in = None;
        }
    }
    let cells: Vec<[i32; 3]> = match &job.crew.access.way_in {
        Some((work, _)) => work.clone(),
        None => cells.to_vec(),
    };
    let cells = cells.as_slice();
    let lo = [
        cells.iter().map(|c| c[0]).min()? - DIG_AROUND,
        cells.iter().map(|c| c[1]).min()? - DIG_BELOW,
        cells.iter().map(|c| c[2]).min()? - DIG_AROUND,
    ];
    let hi = [
        cells.iter().map(|c| c[0]).max()? + DIG_AROUND,
        cells.iter().map(|c| c[1]).max()? + DIG_ABOVE,
        cells.iter().map(|c| c[2]).max()? + DIG_AROUND,
    ];
    let inside = |c: [i32; 3]| (0..3).all(|i| (lo[i]..=hi[i]).contains(&c[i]));
    // Where the golem walks now, of the box: what the way in starts from.
    let here: HashMap<[i32; 3], u32> = match route::region(ctx, body.cell, false, &[]) {
        Some(Some(region)) => region
            .cells()
            .filter(|c| inside(*c))
            .filter_map(|c| Some((c, region.moves(c)?)))
            .collect(),
        _ => return None,
    };
    if here.is_empty() {
        if super::TRACE {
            log("TRACE way in: the golem walks nowhere inside the box");
        }
        job.crew.access.way_in = None;
        return None;
    }
    // Standing room right beside the work, not merely within reach: a block
    // walled into a bank is seen only from the cell next to it.
    let mut goals: HashSet<[i32; 3]> = HashSet::default();
    for work in cells {
        for side in SIDES {
            // Level with it, a step below it, or standing on what is beside
            // it and reaching down — the last is often the only room left
            // once the walls around the work are up.
            for dy in [0, -1, 1] {
                let g = offset(offset(*work, side), [0, dy, 0]);
                if !cells.contains(&g)
                    && !cells.contains(&offset(g, [0, 1, 0]))
                    && inside(g)
                    && !here.contains_key(&g)
                {
                    goals.insert(g);
                }
            }
        }
    }
    // Standing room beside the work the golem can already walk to: the way
    // in is dug, and what kept it from the work was something else.
    if goals.iter().any(|g| here.contains_key(g)) {
        trace!(
            "TRACE way in to {:?}: already beside it, nothing to dig",
            cells[0]
        );
        job.crew.access.way_in = None;
        return None;
    }
    if goals.is_empty() {
        trace!("TRACE way in to {:?}: nowhere to stand", cells[0]);
        job.crew.access.way_in = None;
        return None;
    }
    let size = [hi[0] - lo[0] + 1, hi[1] - lo[1] + 1, hi[2] - lo[2] + 1];
    let grid = ground(ctx, job, project, body, lo, size, true)?;
    let health = mob_info(body.id).map_or(1.0, |m| m.health);
    // Never a fall or a scaffold to get there: this is a way dug, and the
    // golem must be able to walk back out of it the way it came.
    let Some(way) = search(
        &grid,
        &here,
        &goals,
        health,
        &job.crew.access.no_go,
        false,
        false,
    ) else {
        trace!(
            "TRACE way in to {:?}: no way dug from {} cells to {} ways in",
            cells[0],
            here.len(),
            goals.len()
        );
        job.crew.access.way_in = None;
        return None;
    };
    if job.crew.access.way_in.is_none() {
        job.crew.access.way_in = Some((cells.to_vec(), ctx.now));
    }
    trace!(
        "TRACE digging toward {:?}: {} moves, first {:?} from {:?} digging {:?}",
        cells[0],
        way.cost,
        way.step,
        way.from,
        way.digs
    );
    if way.from != body.cell {
        return step_to(ctx, job, body, way.from);
    }
    if let Some(cell) = way.digs.iter().copied().find(|c| !open(ctx, *c)) {
        return Some(Step::Centre {
            task: Task::Breakout(cell),
            since: ctx.now,
        });
    }
    // Nothing to dig on the way: the cell is walked to, and the work is
    // judged again from beside it.
    match way.step {
        Move::Walk(to) | Move::Drop(to) => step_to(ctx, job, body, to),
        Move::Rise | Move::Sink => None,
    }
}

/// Walk one step of a way in once the navigator agrees it can be walked. A
/// refused step is remembered and weighed out of the next way, or the search
/// keeps choosing it.
fn step_to(ctx: &mut Ctx, job: &mut Job, body: &Body, to: [i32; 3]) -> Option<Step> {
    match route::probe(ctx, body.cell, to, Vec::new()) {
        Some(Route::Open) => Some(walk_to(ctx, to, Then::Escape)),
        Some(_) => {
            trace!(
                "TRACE the way in asks for a step from {:?} to {to:?} that no route walks",
                body.cell
            );
            if job.crew.access.no_go.len() >= NO_GO {
                job.crew.access.no_go.remove(0);
            }
            job.crew.access.no_go.push((body.cell, to));
            None
        }
        None => None,
    }
}
