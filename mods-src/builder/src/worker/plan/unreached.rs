//! Work nothing reaches: what is cleared away around it, and cutting a way
//! toward it.

use mod_sdk::*;

use super::{defer_task, Flow, Round};
use crate::geometry::{manhattan, offset};
use crate::jobs::Job;
use crate::project::{Project, Projects};
use crate::worker::tuning::patience::SCAFFOLD_TRIES;
use crate::worker::tuning::reach::{COVER, TRIM_REACH};
use crate::worker::tuning::waits::{OUT_OF_REACH, SEALED_WAIT};
use crate::worker::upkeep::open_block;
use crate::worker::{pocket, wayin, Body, Ctx, Step, Task, TRACE};

/// Mark what lies ON work the golem cannot reach: a block it could otherwise
/// stand beside and dig is still hidden while earth covers it, and nothing
/// the design says anything about is touched.
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
/// Mark the overgrowth within a few blocks of `cells` for cutting: foliage
/// in cells the design does not govern.
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

/// Cutting on toward work ground shuts the golem out of: the way in it began,
/// or the nearest work it has found no way to.
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

/// Work no stance, pillar or roof course reaches: a door in the way is
/// opened, a way is dug, what hides it is cleared, and otherwise it waits.
/// `Some` when that is this tick's step.
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
    // A door shut between the golem and the work opens: no stance beyond it
    // can be walked to while it stands closed.
    if let Some(step) = wayin::door_toward(ctx, job, body, cells) {
        return Some(Flow::Go(step));
    }
    // Out of reach once may be the ground still to be laid under it; out of
    // reach again is work with nothing beside it to stand on, and only
    // digging will do. (Its own pace: one block dug, the next is weighed at
    // once.)
    let again = matches!(task, Task::Unit(i) if job.crew.access.unreachable.contains(&i));
    if again {
        if let Some(step) = wayin::dig_toward(ctx, job, body, project, cells) {
            return Some(Flow::Go(step));
        }
    }
    if let Task::Unit(i) = task {
        job.crew.access.unreachable.insert(i);
        // Propped and still seen from nowhere: the prop is no face a click
        // can use. It comes down, and the unit waits for the build to bring
        // its face — nothing beside it is dug out for it either.
        if let Some(props) = job.crew.faces.unprop(i) {
            trace!("TRACE the props of Unit({i}) gave it no face: taking {props:?} down");
            job.crew.scaffolding.urgent.extend(props);
            job.crew.faces.hangs.insert(i);
        }
        // Sealed in on every side, or hidden from all standing room in reach
        // (a tunnel under a course laid too soon): a built neighbour comes
        // back down to open a way in.
        if !digging && !job.crew.faces.hangs.contains(&i) {
            if let Some(o) = pocket::opener(ctx, job, i, body.cell, unseen) {
                trace!("TRACE {task:?} sealed in: reopening Unit({o})");
                // With what came down for it before: one put back as the next
                // came down opened nothing, and the two were dug and laid in
                // turn for ever.
                let openers = job.crew.access.reopen.entry(i).or_default();
                if !openers.contains(&o) {
                    openers.push(o);
                }
            }
        }
    }
    // Natural overgrowth around work nothing reaches (a tree's canopy over a
    // wall) is cut away, never the design's own.
    if matches!(task, Task::Unit(_)) {
        trim_around(job, cells);
        uncover(ctx, job, cells);
    }
    // A support column that cannot be finished comes down and its unit is
    // planned afresh, or one raised in a bad spot can block its own floor for
    // good.
    if let Task::Support { unit, .. } = task {
        if let Some(props) = job.crew.faces.unprop(unit) {
            trace!("TRACE support for Unit({unit}) unreachable: taking {props:?} down");
            job.crew.scaffolding.urgent.extend(props);
        }
    }
    // Cutting overgrowth back is a courtesy to the work, never work of its
    // own: a clump nothing reaches stays.
    if let Task::Trim(cell) = task {
        job.crew.access.trims.remove(&cell);
        return None;
    }
    // Scaffolding out of reach is tried again for a while (a course coming
    // down may open a way) and then left standing: some can only be dug from
    // inside a pit, and asking forever never finishes the build.
    if let Task::Scaffold(cell) = task {
        let tries = job.crew.scaffolding.shunned.entry(cell).or_default();
        *tries += 1;
        if *tries < SCAFFOLD_TRIES {
            defer_task(ctx, job, task, SEALED_WAIT);
            return None;
        }
        trace!("TRACE leaving the scaffold at {cell:?} standing");
        job.crew.scaffolding.shunned.remove(&cell);
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

/// A way in half dug is finished before the golem walks off for materials:
/// the trip is long, and the corridor is what the work waits on.
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

/// Nothing to lay this tick: rather than stand about until the blocks it
/// cannot reach come round again, the golem cuts on toward them.
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
