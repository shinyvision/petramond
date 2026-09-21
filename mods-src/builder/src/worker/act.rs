//! Doing one task from where the golem stands: a paid placement, a dig on
//! consecutive ticks, and what each refusal means for the plan.

use mod_sdk::*;

use super::tuning::hands::{AIM_SETTLE_TICKS, AIM_TICKS, TURN_PER_TICK};
use super::{hands, sight, Body, Ctx, Step, Task};
use crate::geometry::FACES;
use crate::jobs::Job;
use crate::project::Projects;
use crate::survey::Known;

pub const JAB: &str = "jab";

/// Start aiming at a placement: turn to it, block in hand, and answer when
/// it goes in. `None` for work that is no placement.
pub fn aim(ctx: &mut Ctx, job: &mut Job, body: &Body, task: Task) -> Option<u64> {
    let (cell, item) = match task {
        Task::Unit(i) => match job.survey.as_ref().map(|s| &s.known[i]) {
            Some(Known::Place(missing)) => (
                job.design.units[i].pos,
                missing.first().map(|s| s.item.clone()),
            ),
            _ => return None,
        },
        Task::Support { cell, .. } => (
            cell,
            super::scaffold::pick(ctx, job, body).map(|kind| kind.item.clone()),
        ),
        Task::Scaffold(_) | Task::Reopen(_) | Task::Trim(_) | Task::Breakout(_) => return None,
    };
    job.crew.presence.set_hold(body.id, true);
    job.crew.presence.unaimed = 0;
    // With no look at it from here the placement itself says why.
    let work = sight::work(job, task);
    let _ = sight::turn(ctx, job, body, &work);
    let turn = job.crew.presence.look.map_or(0.0, |yaw| {
        ((yaw - body.yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
            - std::f32::consts::PI)
            .abs()
    });
    let turned = ctx.now + AIM_SETTLE_TICKS + (turn / TURN_PER_TICK) as u64;
    job.crew.presence.hold_item(body.id, item);
    // A hash of the moment and the cell stands in for a roll: the mod keeps
    // no random state, and a replay aims the same way.
    let seed = ctx.now
        ^ (cell[0] as u64).wrapping_mul(0x9E37_79B9)
        ^ (cell[2] as u64).rotate_left(17)
        ^ cell[1] as u64;
    let span = AIM_TICKS.end() - AIM_TICKS.start() + 1;
    let gap = AIM_TICKS.start() + seed.wrapping_mul(0x2545_F491_4F6C_DD1D) % span;
    Some(turned.max(job.crew.pace.placed_at + gap))
}

pub fn begin(
    ctx: &mut Ctx,
    projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    task: Task,
) -> Step {
    match task {
        Task::Scaffold(cell) | Task::Trim(cell) | Task::Breakout(cell) => {
            dig(ctx, projects, job, body, task, cell, ctx.now)
        }
        Task::Reopen(o) => dig(
            ctx,
            projects,
            job,
            body,
            task,
            job.design.units[o].pos,
            ctx.now,
        ),
        Task::Support { cell, .. } => prop(ctx, projects, job, body, task, cell),
        Task::Unit(i) => {
            let Some(known) = job.survey.as_ref().map(|s| s.known[i].clone()) else {
                return Step::Plan;
            };
            match known {
                Known::Place(missing) => place(ctx, projects, job, body, i, &missing),
                Known::Clear {
                    at,
                    holds_items: false,
                    ..
                } => dig(ctx, projects, job, body, task, at, ctx.now),
                _ => Step::Plan,
            }
        }
    }
}

fn place(
    ctx: &mut Ctx,
    projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    i: usize,
    missing: &[ItemStackData],
) -> Step {
    let unit = job.design.units[i];
    let record = job.design.records[unit.record as usize].clone();
    let shown = missing.first().map(|s| s.item.clone());
    match hands::lay(ctx, job, body, unit.pos, record, true, shown) {
        hands::Lay::Turning => Step::Aim {
            task: Task::Unit(i),
            until: ctx.now + 1,
        },
        hands::Lay::Queued => {
            trace!(
                "TRACE place Unit({i}) {} at {:?}",
                job.design.records[unit.record as usize].block,
                unit.pos
            );
            Step::Await {
                task: Task::Unit(i),
                since: ctx.now,
            }
        }
        hands::Lay::Satisfied => {
            settle(ctx, projects, job, Task::Unit(i));
            Step::Plan
        }
        hands::Lay::Refused(refusal) => {
            refused(ctx, job, Task::Unit(i), refusal);
            Step::Plan
        }
    }
}

fn prop(
    ctx: &mut Ctx,
    projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    task: Task,
    cell: [i32; 3],
) -> Step {
    match super::scaffold::lay(ctx, job, body, cell) {
        hands::Lay::Turning => Step::Aim {
            task,
            until: ctx.now + 1,
        },
        hands::Lay::Queued => {
            projects.update(job.id, |p| {
                if !p.scaffolds.contains(&cell) {
                    p.scaffolds.push(cell);
                }
            });
            if let Task::Support { unit, .. } = task {
                job.crew.faces.propped.entry(unit).or_default().push(cell);
            }
            Step::Await {
                task,
                since: ctx.now,
            }
        }
        hands::Lay::Satisfied => Step::Plan,
        hands::Lay::Refused(refusal) => {
            refused(ctx, job, task, refusal);
            Step::Plan
        }
    }
}

pub fn dig(
    ctx: &mut Ctx,
    projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    task: Task,
    cell: [i32; 3],
    since: u64,
) -> Step {
    // Cut overgrowth is not carried off: saplings and sticks would fill the
    // golem's hands.
    let collect = !matches!(task, Task::Trim(_));
    match hands::dig(ctx, job, body, cell, collect) {
        hands::Dig::Turning | hands::Dig::Digging => Step::Dig { task, cell, since },
        hands::Dig::Breaking => Step::Await {
            task,
            since: ctx.now,
        },
        hands::Dig::Nothing => {
            // Nothing there to take down: asked again it answers the same,
            // every tick, for ever.
            if let Task::Reopen(o) = task {
                job.crew.access.reopen.retain(|_, openers| {
                    openers.retain(|opener| *opener != o);
                    !openers.is_empty()
                });
            }
            settle(ctx, projects, job, task);
            Step::Plan
        }
        hands::Dig::Refused(refusal) => {
            refused(ctx, job, task, refusal);
            Step::Plan
        }
    }
}

/// Re-read a task's target after the world answered it.
pub fn settle(ctx: &mut Ctx, projects: &mut Projects, job: &mut Job, task: Task) {
    if let Some((_, work)) = job.crew.aloft.climbed_for.as_mut() {
        *work += 1;
    }
    job.crew.pace.progress_at = ctx.now;
    // Work landing answers whatever went wrong before it; a problem that
    // still holds is noted again by the next plan.
    job.crew.note.clear();
    match task {
        Task::Unit(i) => {
            if job
                .crew
                .pace
                .band_top
                .is_none_or(|top| job.design.units[i].pos[1] <= top)
            {
                job.crew.pace.band_progress_at = ctx.now;
            }
            if let Some(survey) = job.survey.as_mut() {
                let placing = matches!(survey.known[i], Known::Place(_));
                survey.measure(&job.design, &[i]);
                if !survey.known[i].open() {
                    if placing {
                        job.crew.built.insert(i);
                        job.crew.deferrals.tried.clear();
                        // Work of another kind landing puts the glazing back
                        // to the end of the build, where it belongs.
                        if !job.design.glazing(i) {
                            job.crew.glazing.under_way = false;
                        }
                        // A door just laid is left standing open while there
                        // is work on its far side.
                        if job.design.passage(i) {
                            job.crew.access.pending_use = Some(job.design.units[i].pos);
                        }
                    }
                    job.crew.access.unreachable.remove(&i);
                    if let Some(props) = job.crew.faces.propped.remove(&i) {
                        job.crew.scaffolding.urgent.extend(props);
                    }
                    // Neighbours waiting for something to hang on or lean
                    // against may go now.
                    for cell in job.design.cells(job.design.units[i]) {
                        for face in FACES {
                            if let Some(n) = job.design.unit_at(crate::geometry::offset(cell, face))
                            {
                                job.crew.deferrals.deferred.remove(&Task::Unit(n));
                            }
                        }
                    }
                }
            }
        }
        Task::Scaffold(cell) => {
            if super::scaffold::stands(ctx.content, cell) == Some(false) {
                projects.update(job.id, |p| p.scaffolds.retain(|c| *c != cell));
            }
        }
        Task::Reopen(o) => {
            if let Some(survey) = job.survey.as_mut() {
                survey.measure(&job.design, &[o]);
                if survey.known[o].open() {
                    job.crew.built.remove(&o);
                    job.crew.pace.cursor = job.crew.pace.cursor.min(o);
                    // What it sealed in is worth every stance again.
                    let sealed: Vec<usize> = job
                        .crew
                        .access
                        .reopen
                        .iter()
                        .filter(|(_, openers)| openers.contains(&o))
                        .map(|(sealed, _)| *sealed)
                        .collect();
                    for sealed in sealed {
                        let task = Task::Unit(sealed);
                        job.crew.access.unreachable.remove(&sealed);
                        job.crew.deferrals.deferred.remove(&task);
                        job.crew.deferrals.blind.retain(|(t, _)| *t != task);
                    }
                }
            }
        }
        Task::Trim(cell) => {
            job.crew.access.trims.remove(&cell);
        }
        // A design block dug to get out goes back up: it is no loss.
        Task::Breakout(cell) => {
            job.crew.access.digs.remove(&cell);
            // Breaking the build is the escape ladder's business, never the
            // way in's: if one is ever broken here, say so loudly.
            if super::TRACE {
                if let Some(i) = job.design.unit_at(cell) {
                    if job.crew.built.contains(&i) || job.crew.rescue.stuck.is_none() {
                        log(&format!(
                            "TRACE BROKE THE BUILD at {cell:?} (Unit({i})), stuck {}",
                            job.crew.rescue.stuck.is_some()
                        ));
                    }
                }
            }
            if let Some(stuck) = job.crew.rescue.stuck.as_mut() {
                stuck.dug(ctx.now);
            }
            if let (Some(i), Some(survey)) = (job.design.unit_at(cell), job.survey.as_mut()) {
                survey.measure(&job.design, &[i]);
                if survey.known[i].open() {
                    job.crew.built.remove(&i);
                    job.crew.pace.cursor = job.crew.pace.cursor.min(i);
                }
            }
        }
        Task::Support { .. } => {}
    }
    if let Some(id) = job.crew.mob {
        job.crew.presence.animate(id, None);
    }
}

fn refused(ctx: &mut Ctx, job: &mut Job, task: Task, refusal: ActionRefusal) {
    if super::TRACE {
        let at = match task {
            Task::Unit(i) => job.design.units[i].pos,
            Task::Scaffold(c) | Task::Support { cell: c, .. } => c,
            Task::Reopen(o) => job.design.units[o].pos,
            Task::Trim(c) | Task::Breakout(c) => c,
        };
        let golem = job.crew.mob.and_then(mob_info).map(|m| m.pos);
        log(&format!(
            "TRACE refused {task:?} at {at:?} golem {golem:?} {refusal:?}"
        ));
    }
    job.crew.presence.unaimed = 0;
    if matches!(
        refusal,
        ActionRefusal::OutOfReach
            | ActionRefusal::NoLineOfSight
            | ActionRefusal::Misaligned
            | ActionRefusal::NotAimed
    ) {
        // The cell it STANDS in: leaning out over an edge, its feet floor to
        // the air beside.
        if let Some(info) = job.crew.mob.and_then(mob_info) {
            job.crew
                .deferrals
                .blind
                .insert((task, super::standing_cell(info.pos)));
        }
    }
    if let Some(id) = job.crew.mob {
        job.crew.presence.animate(id, None);
    }
    let i = match task {
        Task::Unit(i) | Task::Support { unit: i, .. } => i,
        Task::Scaffold(_) | Task::Reopen(_) | Task::Trim(_) | Task::Breakout(_) => {
            job.crew.deferrals.defer(task, ctx.now + 40);
            return;
        }
    };
    // Usually a neighbour in the same layer lands soon and gives the cell a
    // face; only a cell that stays faceless gets scaffolding under it.
    if refusal == ActionRefusal::NoFace && matches!(task, Task::Unit(_)) {
        let fragile = job.design.fragile(job.design.units[i]);
        if job.crew.faces.faceless_try(i, ctx.now) && !fragile {
            job.crew.faces.floating.insert(i);
        } else {
            job.crew.deferrals.defer(task, ctx.now + 120);
        }
        return;
    }
    let (wait, note) = match refusal {
        ActionRefusal::OutOfReach
        | ActionRefusal::NoLineOfSight
        | ActionRefusal::Misaligned
        | ActionRefusal::NotAimed => (10, ""),
        ActionRefusal::BodyInTheWay => (40, "Someone is standing where a block goes"),
        ActionRefusal::NoFace | ActionRefusal::NoSupport => (100, ""),
        ActionRefusal::Unloaded => (100, "Part of the site is not loaded"),
        ActionRefusal::MissingItems => (0, ""),
        ActionRefusal::Obstructed | ActionRefusal::NothingToDo | ActionRefusal::Changed => {
            if let Some(survey) = job.survey.as_mut() {
                survey.measure(&job.design, &[i]);
            }
            (0, "")
        }
        ActionRefusal::Unbreakable => (2400, "Something unbreakable is in the way"),
        ActionRefusal::Vetoed | ActionRefusal::NotOwned | ActionRefusal::Unsupported => {
            (1200, "A block was refused")
        }
        ActionRefusal::NoTool | ActionRefusal::NoActor => (20, ""),
    };
    if wait > 0 {
        job.crew.deferrals.defer(task, ctx.now + wait);
    }
    if !note.is_empty() {
        job.crew.note = note.into();
    }
}

/// An `actor_acted` outcome for this job's golem.
pub fn acted(
    ctx: &mut Ctx,
    projects: &mut Projects,
    job: &mut Job,
    pos: [i32; 3],
    action: ActorAction,
    refusal: Option<ActionRefusal>,
) {
    // A placed block closes routes and a dug one opens them: each forgets the
    // answers it may have made wrong, a dig only those near it.
    if refusal.is_none() {
        ctx.regions.clear();
    }
    if super::TRACE && (refusal.is_some() || matches!(job.crew.step, Step::Bridge { .. })) {
        log(&format!(
            "TRACE outcome {action:?} at {pos:?} refused {refusal:?} during {:?}",
            job.crew.step
        ));
    }
    match (action, refusal) {
        (ActorAction::Place, None) => ctx.routes.retain(|_, (route, _)| *route != Route::Open),
        (ActorAction::Dig, None) => {
            let near = |c: [i32; 3]| crate::geometry::manhattan(c, pos) <= 8;
            ctx.routes.retain(|(from, to, _), (route, _)| {
                *route == Route::Open || !(near(*from) || near(*to))
            });
            projects.update(job.id, |p| p.scaffolds.retain(|c| *c != pos));
        }
        _ => {}
    }
    let Step::Await { task, .. } = job.crew.step else {
        return;
    };
    let target = match task {
        Task::Unit(i) | Task::Reopen(i) => job.design.cells(job.design.units[i]),
        Task::Trim(c) | Task::Breakout(c) => vec![c],
        Task::Scaffold(cell) | Task::Support { cell, .. } => vec![cell],
    };
    let at_unit = match (task, job.survey.as_ref()) {
        (Task::Unit(i), Some(s)) => matches!(s.known[i], Known::Clear { at, .. } if at == pos),
        _ => false,
    };
    if !target.contains(&pos) && !at_unit {
        return;
    }
    match refusal {
        None => settle(ctx, projects, job, task),
        Some(refusal) => refused(ctx, job, task, refusal),
    }
    job.crew.step = Step::Plan;
}
