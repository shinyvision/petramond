use crate::host::prelude::*;

use super::{defer_task, Flow, Round};
use crate::geometry::{manhattan, offset, FACES};
use crate::project::{Project, Projects};
use crate::survey::Known;
use crate::worker::crew::Room;
use crate::worker::route;
use crate::worker::tuning::patience::{
    ROOM_AFTER, ROOM_PATIENCE, ROOM_REACHES, ROOM_ROUND, ROOM_ROUNDS, SCAFFOLD_TRIES,
};
use crate::worker::tuning::reach::{COVER, TRIM_REACH};
use crate::worker::tuning::waits::{OUT_OF_REACH, SEALED_WAIT};
use crate::worker::upkeep::open_block;
use crate::worker::Job;
use crate::worker::{wayin, Body, Ctx, Step, Task, TRACE};

pub(super) fn uncover(ctx: &mut Ctx, job: &mut Job, cells: &[[i32; 3]]) {
    let mut over: Vec<[i32; 3]> = Vec::new();
    for cell in cells {
        for up in 1..=COVER {
            let c = offset(*cell, [0, up, 0]);
            if job.design.governed.contains(&c)
                || job.design.unit_at(c).is_some()
                || job.crew.access.digs.contains(&c)
            {
                continue;
            }
            over.push(c);
        }
    }
    if over.is_empty() {
        return;
    }
    let blocks = get_blocks(over.clone());
    for (cell, block) in over.into_iter().zip(blocks) {
        let Some(block) = block else { continue };
        if open_block(ctx, block) {
            continue;
        }
        if job.crew.access.digs.insert(cell) && TRACE {
            log(&format!("TRACE earth over the work at {cell:?} comes off"));
        }
    }
}
pub(super) fn trim_around(job: &mut Job, cells: &[[i32; 3]]) {
    let overgrowth = job.design.overgrowth();
    if overgrowth.is_empty() || cells.is_empty() {
        return;
    }
    let min: [i32; 3] =
        std::array::from_fn(|i| cells.iter().map(|c| c[i]).min().unwrap_or(0) - TRIM_REACH);
    let max: [i32; 3] =
        std::array::from_fn(|i| cells.iter().map(|c| c[i]).max().unwrap_or(0) + TRIM_REACH);
    let Some(found) = find_blocks(min, max, overgrowth) else {
        return;
    };
    for cell in found {
        if !job.design.governed.contains(&cell) && job.crew.access.trims.insert(cell) && TRACE {
            log(&format!("TRACE trimming overgrowth at {cell:?}"));
        }
    }
}

pub(super) fn digging_on(
    ctx: &mut Ctx,
    job: &mut Job,
    body: &Body,
    project: &crate::project::Project,
) -> Option<Step> {
    let cells: Vec<[i32; 3]> = match &job.crew.access.way_in {
        Some((work, _)) => work.clone(),
        None => {
            let opening = job.access_plan.score().is_some_and(|n| n > 0);
            let mut shut_out: Vec<[i32; 3]> = if opening {
                job.access_plan
                    .sealed_units()
                    .iter()
                    .map(|i| job.design.units[*i].pos)
                    .collect()
            } else {
                let survey = job.survey.as_ref()?;
                job.crew
                    .access
                    .unreachable
                    .iter()
                    .filter(|i| survey.known[**i].open())
                    .map(|i| job.design.units[*i].pos)
                    .collect()
            };
            shut_out.sort_by_key(|c| (manhattan(*c, body.cell), *c));
            shut_out.dedup();
            if opening {
                shut_out
            } else {
                vec![*shut_out.first()?]
            }
        }
    };
    if cells.is_empty() {
        return None;
    }
    wayin::dig_toward(ctx, job, body, project, &cells)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn fall_out(
    ctx: &mut Ctx,
    projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    project: &Project,
    task: Task,
    cells: &[[i32; 3]],
) -> Option<Flow> {
    if let Some(step) = wayin::door_toward(ctx, job, body, cells) {
        return Some(Flow::Go(step));
    }
    let again = matches!(task, Task::Unit(i) if job.crew.access.unreachable.contains(&i));
    if again {
        if let Some(step) = wayin::dig_toward(ctx, job, body, project, cells) {
            return Some(Flow::Go(step));
        }
    }
    if let Task::Unit(i) = task {
        job.crew.access.unreachable.insert(i);
        if let Some(props) = job.crew.faces.unprop(i) {
            trace!("TRACE the props of Unit({i}) gave it no face: taking {props:?} down");
            job.crew.scaffolding.urgent.extend(props);
            job.crew.faces.hangs.insert(i);
        }
    }
    if matches!(task, Task::Unit(_)) {
        trim_around(job, cells);
        uncover(ctx, job, cells);
    }
    if let Task::Support { unit, .. } = task {
        if let Some(props) = job.crew.faces.unprop(unit) {
            trace!("TRACE support for Unit({unit}) unreachable: taking {props:?} down");
            job.crew.scaffolding.urgent.extend(props);
        }
    }
    if let Task::Trim(cell) = task {
        job.crew.access.trims.remove(&cell);
        return None;
    }
    if let Task::Scaffold(cell) = task {
        if job.crew.scaffolding.shunned.count(cell) < SCAFFOLD_TRIES {
            defer_task(ctx, job, task, SEALED_WAIT);
            return None;
        }
        trace!("TRACE leaving the scaffold at {cell:?} standing");
        job.crew.scaffolding.shunned.forget(&cell);
        job.crew.scaffolding.left_standing += 1;
        job.crew.scaffolding.left.insert(cell);
        job.crew.scaffolding.urgent.retain(|c| *c != cell);
        projects.update(job.id, |p| p.scaffolds.retain(|c| *c != cell));
        return None;
    }
    job.crew.note = "Some blocks are out of the golem's reach".into();
    trace!(
        "TRACE unreachable {task:?} cells {cells:?} from {:?}",
        body.cell
    );
    defer_task(ctx, job, task, OUT_OF_REACH);
    None
}

pub(super) fn finish_way_in(
    ctx: &mut Ctx,
    _projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    round: &mut Round,
) -> Flow {
    let project = &round.project;
    if job.crew.access.way_in.is_some() {
        if let Some(step) = digging_on(ctx, job, body, project) {
            return Flow::Go(step);
        }
    }
    Flow::Pass
}

pub(super) fn dig_on(
    ctx: &mut Ctx,
    _projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    round: &mut Round,
) -> Flow {
    let project = &round.project;
    if let Some(step) = digging_on(ctx, job, body, project) {
        return Flow::Go(step);
    }
    Flow::Pass
}

/// Work no stance can place gets room from another side, as a player would make it: a finished
/// neighbour the worker can get at comes down, the work goes in through the gap, and the neighbour
/// is built again. Each neighbour is tried once per piece of work.
pub(super) fn make_room(
    ctx: &mut Ctx,
    _projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    _round: &mut Round,
) -> Flow {
    if let Some(room) = job.crew.access.room {
        if ctx.now < room.since + ROOM_PATIENCE {
            return Flow::Pass;
        }
        // A neighbour never taken down was no try: the worker could not get to it in time, and
        // it is offered again a few times.
        let taken_down = job
            .survey
            .as_ref()
            .is_some_and(|s| !matches!(s.known[room.through], Known::Satisfied));
        let pair = (room.unit, room.through);
        if taken_down || !job.crew.access.unreached_rooms.within(pair, ROOM_REACHES) {
            trace!(
                "TRACE room through Unit({}) did not get Unit({}) placed",
                room.through,
                room.unit
            );
            job.crew.access.roomed.insert(pair);
        }
        job.crew.access.reopened.remove(&room.through);
        job.crew.access.room = None;
    }
    if ctx.now < job.crew.pace.progress_at + ROOM_AFTER || job.access_plan.score() != Some(0) {
        return Flow::Pass;
    }
    let Some(survey) = job.survey.as_ref() else {
        return Flow::Pass;
    };
    let mut stuck: Vec<usize> = job
        .crew
        .access
        .unreachable
        .iter()
        .chain(&job.crew.access.abandoned)
        .copied()
        .filter(|i| matches!(survey.known[*i], Known::Place(_)) && !job.crew.built.contains(i))
        .collect();
    stuck.sort_unstable_by_key(|i| (manhattan(job.design.units[*i].pos, body.cell), *i));
    stuck.dedup();
    let Some(region) = route::region(ctx, body.cell, false, &[]).flatten() else {
        return Flow::Pass;
    };
    let approachable = |cell: [i32; 3], beside: &[[i32; 3]]| {
        FACES.iter().any(|f| {
            let m = offset(cell, *f);
            !beside.contains(&m) && (region.contains(m) || region.contains(offset(m, [0, -1, 0])))
        })
    };
    let mut spent = Vec::new();
    for unit in stuck {
        let cells = job.design.cells(job.design.units[unit]);
        let mut through: Vec<(bool, bool, usize)> = Vec::new();
        for cell in &cells {
            for face in FACES {
                let n = offset(*cell, face);
                let Some(j) = job.design.unit_at(n) else {
                    continue;
                };
                let u = job.design.units[j];
                let why = if cells.contains(&n) {
                    "own"
                } else if !matches!(survey.known[j], Known::Satisfied) {
                    "open"
                } else if !matches!(job.design.plan(u), crate::design::Plan::Build { .. })
                    || job.design.cells(u).len() != 1
                    || job.design.fragile(u)
                {
                    "kept"
                } else if !job.access_plan.can_clear(n) {
                    "not clearable"
                } else if job.crew.access.roomed.contains(&(unit, j)) {
                    "tried"
                } else if through.iter().any(|(_, _, k)| *k == j) {
                    "listed"
                } else {
                    ""
                };
                if !why.is_empty() {
                    trace!("TRACE room for Unit({unit}) not through Unit({j}) at {n:?}: {why}");
                    continue;
                }
                let plain = job.design.records[u.record as usize].state.is_empty();
                through.push((!approachable(n, &cells), !plain, j));
            }
        }
        through.sort_unstable();
        if through.is_empty() {
            spent.push(unit);
            continue;
        }
        if let Some(&(_, _, j)) = through.first() {
            trace!(
                "TRACE making room for Unit({unit}) at {:?} through Unit({j}) at {:?}",
                job.design.units[unit].pos,
                job.design.units[j].pos
            );
            job.crew.access.room = Some(Room {
                unit,
                through: j,
                since: ctx.now,
            });
            return Flow::Pass;
        }
    }
    let now = ctx.now;
    for unit in spent {
        spent_rooms(now, job, unit);
    }
    Flow::Pass
}

/// Every neighbour of `unit` has had its turn as a room. After a while they get another turn, the
/// world around having changed since; after the last round the work is given up on.
fn spent_rooms(now: u64, job: &mut Job, unit: usize) {
    let access = &mut job.crew.access;
    if !access.roomed.iter().any(|(u, _)| *u == unit) {
        return;
    }
    let since = *access.rooms_spent.entry(unit).or_insert(now);
    if now < since + ROOM_ROUND {
        return;
    }
    access.rooms_spent.remove(&unit);
    if access.room_rounds.within(unit, ROOM_ROUNDS) {
        access.roomed.retain(|(u, _)| *u != unit);
        access.unreached_rooms.retain(|(u, _)| *u != unit);
    } else {
        trace!("TRACE giving up on Unit({unit})");
        access.given_up.insert(unit);
    }
}
