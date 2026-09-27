use crate::host::prelude::*;

use super::{defer_task, Flow, Round};
use crate::geometry::{manhattan, offset};
use crate::project::{Project, Projects};
use crate::worker::tuning::patience::SCAFFOLD_TRIES;
use crate::worker::tuning::reach::{COVER, TRIM_REACH};
use crate::worker::tuning::waits::{OUT_OF_REACH, SEALED_WAIT};
use crate::worker::upkeep::open_block;
use crate::worker::Job;
use crate::worker::{pocket, wayin, Body, Ctx, Step, Task, TRACE};

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
            let survey = job.survey.as_ref()?;
            let mut shut_out: Vec<[i32; 3]> = job
                .crew
                .access
                .unreachable
                .iter()
                .filter(|i| survey.known[**i].open())
                .map(|i| job.design.units[*i].pos)
                .collect();
            shut_out.sort_by_key(|c| (manhattan(*c, body.cell), *c));
            vec![*shut_out.first()?]
        }
    };
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
    unseen: bool,
    cells: &[[i32; 3]],
    digging: bool,
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
        if !digging && !job.crew.faces.hangs.contains(&i) {
            if let Some(o) = pocket::opener(ctx, job, i, body.cell, unseen) {
                trace!("TRACE {task:?} sealed in: reopening Unit({o})");
                let openers = job.crew.access.reopen.entry(i).or_default();
                if !openers.contains(&o) {
                    openers.push(o);
                }
            }
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
