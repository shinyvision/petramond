//! Gathering the open work a plan weighs: what can be done with what is
//! carried, within the layers being worked.

use mod_sdk::*;

use super::glazing::{only_glazing_left, ways_through};
use super::sealing::{only_cutting_work_left, swings_open};
use super::{trace, Flow, Mode, Round};
use crate::fx::HashSet;
use crate::geometry::{manhattan, offset, FACES};
use crate::jobs::Job;
use crate::project::Projects;
use crate::survey::Known;
use crate::worker::crew::Stamped;
use crate::worker::route::{self, Hubs};
use crate::worker::tuning::every::{HELD_EVERY, WAYS_EVERY};
use crate::worker::tuning::patience::{BAND_PATIENCE, GLAZE_PATIENCE};
use crate::worker::tuning::waits::OUT_OF_REACH;
use crate::worker::tuning::window::{LAYER_BAND, PERCH_REACH, SCAN, STAY_REACH, WINDOW};
use crate::worker::upkeep::open_block;
use crate::worker::{cargo, support, Body, Ctx, Task};

/// The open work a plan weighs.
#[derive(Default)]
pub(super) struct Candidates {
    /// Tasks doable with what is carried, and where each is.
    pub(super) list: Vec<(Task, [i32; 3])>,
    /// Whether any placement waits on items.
    pub(super) wants_items: bool,
    /// Whether any work waits on time.
    pub(super) waiting: bool,
}

impl Candidates {
    /// Take `task` up, or note that it waits where it has been set aside.
    fn offer(&mut self, deferred: bool, task: Task, cell: [i32; 3]) {
        if deferred {
            self.waiting = true;
        } else {
            self.list.push((task, cell));
        }
    }
}

/// The open work in the window: tasks doable with what is carried, whether
/// any placement waits on items, and whether any work waits on time.
pub(super) fn gather(
    ctx: &mut Ctx,
    job: &mut Job,
    body: &Body,
    project: &crate::project::Project,
    mode: Mode,
    ceiling: Option<i32>,
) -> Candidates {
    let mut found = Candidates::default();
    // Ways in stay open until they are all that is left to build; windows
    // are glazed last, and are ways through until then.
    let closing = only_cutting_work_left(ctx, job, None);
    // Once the wait has passed, glazing goes in until something else lands,
    // rather than waiting again before every pane.
    let idle = ctx.now.saturating_sub(job.crew.pace.progress_at);
    if idle > GLAZE_PATIENCE {
        job.crew.glazing.under_way = true;
    }
    // What the golem can lay hands on (its slots and the chests), re-read now
    // and then: reading every chest is dear and their contents change slowly.
    if job.crew.cargo.in_reach.stale(ctx.now, HELD_EVERY) {
        let stock = ctx.supplies.stock(project.table);
        // Chests out of the loaded world read as empty: what they hold is
        // unknown, not nothing, and no work is passed over for it.
        let held = stock.read.then(|| {
            let mut held = cargo::totals(&body.slots);
            for (key, count) in stock.totals {
                *held.entry(key).or_default() += count;
            }
            held
        });
        job.crew.cargo.in_reach = Stamped {
            at: ctx.now,
            value: held,
        };
    }
    let held = job.crew.cargo.in_reach.value.clone();
    let glazing = job.crew.glazing.under_way || only_glazing_left(job, held.as_ref());
    if !glazing && job.crew.glazing.ways.stale(ctx.now, WAYS_EVERY) {
        job.crew.glazing.ways = Stamped {
            at: ctx.now,
            value: ways_through(job, ceiling),
        };
    }
    let Some(survey) = job.survey.as_ref() else {
        found.waiting = true;
        return found;
    };
    let crew = &mut job.crew;
    while crew.pace.cursor < survey.known.len() && !survey.known[crew.pace.cursor].open() {
        crew.pace.cursor += 1;
    }
    let carried = cargo::totals(&body.slots);
    // Only a building golem lays anything: a job on its way home only takes
    // its scaffolding down.
    let scan = if mode != Mode::Building { 0 } else { SCAN };
    // Judged against all unbuilt work, not the window: the window shrinks as
    // deferrals lapse, and a support whose unit fell out of it was dug as
    // stray.
    let open_cells: HashSet<[i32; 3]> = survey
        .known
        .iter()
        .enumerate()
        .skip(crew.pace.cursor)
        .take(scan)
        .filter(|(i, known)| matches!(known, Known::Place(_)) && !crew.built.contains(i))
        .map(|(i, _)| job.design.units[i].pos)
        .collect();
    trace::held_ways_in(ctx, &job.design, survey, crew, closing);
    // Earth comes off from its open face inward: a block with nothing open
    // beside it is seen from nowhere, so it waits until digging lays it bare.
    let bare = exposed(
        ctx,
        survey
            .known
            .iter()
            .skip(crew.pace.cursor)
            .take(scan)
            .filter_map(|known| match known {
                Known::Clear {
                    at,
                    holds_items: false,
                    ..
                } => Some(*at),
                _ => None,
            })
            .collect(),
    );
    let mut buried: Vec<(Task, [i32; 3])> = Vec::new();
    let mut open = 0;
    for (i, known) in survey
        .known
        .iter()
        .enumerate()
        .skip(crew.pace.cursor)
        .take(scan)
    {
        // With no band to bound it (nothing landing), the first few in build
        // order are all a plan can afford to weigh.
        if ceiling.is_none() && open == WINDOW {
            break;
        }
        if !known.open() {
            continue;
        }
        // Above the layers being worked: it waits its turn — unless the
        // golem is up its scaffolding right beside it. A pillar is dear: what
        // it reaches is laid from it, whatever its layer, before it comes down
        // to be raised again in the same spot once the band gets there.
        if ceiling.is_some_and(|top| job.design.units[i].pos[1] > top)
            && !beside_the_perch(crew, body, job.design.units[i].pos)
        {
            found.waiting = true;
            continue;
        }
        trace::passage(ctx, &job.design, crew, i, known, &carried, closing);
        if crew.deferrals.deferred(Task::Unit(i), ctx.now) {
            found.waiting = true;
            continue;
        }
        open += 1;
        match known {
            Known::Place(_) if crew.built.contains(&i) => {
                // Built once and gone again (decayed, broken): reported, never
                // rebuilt for free — a recheck is the owner's call.
                open -= 1;
            }
            Known::Place(_)
                if job.design.passage(i)
                    && !closing
                    && !swings_open(ctx.caches, &job.design, i) =>
            {
                open -= 1;
                found.waiting = true;
            }
            Known::Place(_)
                if job.design.glazing(i)
                    && !glazing
                    && !crew.glazing.ahead.contains(&i)
                    && crew.glazing.ways.value.contains(&i) =>
            {
                open -= 1;
                found.waiting = true;
            }
            // Blocks neither the hands nor the chests hold wait, giving their
            // place in the window to work that can go up.
            Known::Place(missing)
                if held
                    .as_ref()
                    .is_some_and(|held| !cargo::holds(held, missing)) =>
            {
                open -= 1;
                found.wants_items = true;
            }
            Known::Place(missing) => {
                if !cargo::holds(&carried, missing) {
                    found.wants_items = true;
                    continue;
                }
                let pos = job.design.units[i].pos;
                if leans_on_scaffold(&job.design, job.design.units[i], &project.scaffolds) {
                    found.waiting = true;
                    continue;
                }
                if !crew.faces.floating.contains(&i) {
                    found.list.push((Task::Unit(i), pos));
                    continue;
                }
                match support::below(ctx, &job.design, pos) {
                    support::Support::Needed(cell) => {
                        let task = Task::Support { unit: i, cell };
                        found.offer(crew.deferrals.deferred(task, ctx.now), task, cell);
                    }
                    support::Support::Standing => {
                        crew.faces.floating.remove(&i);
                        found.list.push((Task::Unit(i), pos));
                    }
                    support::Support::Impossible => {
                        crew.note = "Some blocks have nothing to be placed against".into();
                        trace!("TRACE no support under {pos:?}");
                        crew.faces.floating.remove(&i);
                        crew.deferrals.defer(Task::Unit(i), ctx.now + OUT_OF_REACH);
                    }
                }
            }
            // The golem's own scaffolding standing in a room comes down as
            // scaffolding does, in its turn: never dug as an obstruction.
            Known::Clear { at, .. } if crew.scaffolding.cells.contains(at) => {
                open -= 1;
            }
            Known::Clear {
                at,
                holds_items: false,
                ..
            } => {
                if !bare.contains(at) {
                    open -= 1;
                    buried.push((Task::Unit(i), *at));
                } else if !crew.aloft.perch.is_some_and(|p| p.holds(*at)) {
                    found.list.push((Task::Unit(i), *at));
                }
            }
            Known::Clear { .. } => crew.note = "A container with items is in the way".into(),
            Known::Unloaded | Known::Unchecked => found.waiting = true,
            _ => {}
        }
    }
    // Earth nothing will ever lay bare (an eave run into the bank, with no
    // room beside it) is found a way to once nothing else is left to do.
    if found.list.is_empty() {
        found.list.append(&mut buried);
    } else if !buried.is_empty() {
        found.waiting = true;
    }
    // Stray scaffolding comes down only on the walk home or from the top of the
    // golem's own pillar: anywhere else up high a stray may be the way down.
    let on_top = crew
        .aloft
        .perch
        .filter(|p| body.cell == p.top_cell() && crew.aloft.bridge.is_none());
    let walks_home = crew.aloft.perch.is_none() && !project.scaffolds.is_empty() && {
        let trail = crew.trail.clone();
        let hubs = Hubs::new(project.home, &trail);
        matches!(route::out(ctx, hubs, body.cell, &[]), Some(Route::Open))
    };
    let crew = &mut job.crew;
    if walks_home || on_top.is_some() {
        crew.scaffolding
            .urgent
            .retain(|c| project.scaffolds.contains(c));
        for cell in &project.scaffolds {
            if on_top.is_some_and(|p| p.on_column(*cell)) {
                continue;
            }
            // Scaffolding under the golem or beside and below it may be its
            // own way down: a pillar dug out from the roof beside it
            // stranded the golem up there.
            if (cell[0] - body.cell[0]).abs() <= 1
                && (cell[2] - body.cell[2]).abs() <= 1
                && cell[1] < body.cell[1]
            {
                continue;
            }
            let task = Task::Scaffold(*cell);
            if crew.deferrals.deferred(task, ctx.now) {
                found.waiting = true;
            } else if crew.scaffolding.urgent.contains(cell)
                || !support::props_up(*cell, &open_cells)
            {
                found.list.push((task, *cell));
            }
        }
    }
    crew.access
        .reopen
        .retain(|sealed, _| matches!(survey.known[*sealed], Known::Place(_)));
    let mut reopen: Vec<usize> = crew
        .access
        .reopen
        .values()
        .flatten()
        .copied()
        .filter(|o| matches!(survey.known[*o], Known::Satisfied))
        .collect();
    reopen.sort_unstable();
    reopen.dedup();
    for o in reopen {
        let task = Task::Reopen(o);
        found.offer(
            crew.deferrals.deferred(task, ctx.now),
            task,
            job.design.units[o].pos,
        );
    }
    // Earth lying on work out of reach: taken off like any other job, so the
    // nearest of it goes first and the rest waits its turn.
    if !crew.access.digs.is_empty() {
        let mut digs: Vec<[i32; 3]> = crew.access.digs.iter().copied().collect();
        digs.sort_unstable();
        let blocks = get_blocks(digs.clone());
        for (cell, block) in digs.into_iter().zip(blocks) {
            match block {
                Some(b) if !open_block(ctx, b) => {
                    let task = Task::Breakout(cell);
                    found.offer(crew.deferrals.deferred(task, ctx.now), task, cell);
                }
                Some(_) => {
                    crew.access.digs.remove(&cell);
                }
                None => found.waiting = true,
            }
        }
    }
    if !crew.access.trims.is_empty() {
        let overgrowth = job.design.overgrowth();
        let mut trims: Vec<[i32; 3]> = crew.access.trims.iter().copied().collect();
        trims.sort_unstable();
        let blocks = get_blocks(trims.clone());
        let crew = &mut job.crew;
        for (cell, block) in trims.into_iter().zip(blocks) {
            match block {
                Some(b) if overgrowth.contains(&b) => {
                    let task = Task::Trim(cell);
                    found.offer(crew.deferrals.deferred(task, ctx.now), task, cell);
                }
                Some(_) => {
                    crew.access.trims.remove(&cell);
                }
                None => found.waiting = true,
            }
        }
    }
    found
}

fn exposed(ctx: &mut Ctx, cells: Vec<[i32; 3]>) -> HashSet<[i32; 3]> {
    let solid: HashSet<[i32; 3]> = cells.iter().copied().collect();
    let mut beside: Vec<[i32; 3]> = crate::geometry::beside(&cells)
        .filter(|n| !solid.contains(n))
        .collect();
    beside.sort_unstable();
    beside.dedup();
    let open: HashSet<[i32; 3]> = beside
        .iter()
        .zip(crate::design::paged(beside.clone(), get_blocks))
        .filter(|(_, b)| b.is_some_and(|b| open_block(ctx, b)))
        .map(|(c, _)| *c)
        .collect();
    cells
        .into_iter()
        .filter(|c| FACES.iter().any(|f| open.contains(&offset(*c, *f))))
        .collect()
}

/// The layer of the lowest work still to do, waiting or not: placements not
/// yet built (ways in excepted, they are held to the end) and clearances.
pub(super) fn work_floor(job: &Job) -> Option<i32> {
    let survey = job.survey.as_ref()?;
    survey
        .known
        .iter()
        .enumerate()
        .skip(job.crew.pace.cursor)
        .find(|(i, k)| match k {
            Known::Place(_) => {
                !job.crew.built.contains(i)
                    && !job.design.passage(*i)
                    && !job.design.glazing(*i)
                    && !job.crew.faces.waits_on_the_build(*i)
            }
            Known::Clear { holds_items, .. } => !holds_items,
            _ => false,
        })
        .map(|(i, _)| job.design.units[i].pos[1])
}

/// Whether a fragile block (a lantern) would hang on one of the golem's own
/// scaffolds, and so break when the scaffold comes down. Sturdy shapes (a
/// fence on a support) stand on their own.
pub(super) fn leans_on_scaffold(
    design: &crate::design::Design,
    unit: crate::design::Unit,
    scaffolds: &[[i32; 3]],
) -> bool {
    if scaffolds.is_empty()
        || !matches!(
            design.plan(unit),
            crate::design::Plan::Build { fragile: true, .. }
        )
    {
        return false;
    }
    design
        .cells(unit)
        .iter()
        .any(|c| FACES.iter().any(|f| scaffolds.contains(&offset(*c, *f))))
}

/// Whether the golem is up its scaffolding with `cell` beside it.
fn beside_the_perch(crew: &crate::worker::crew::Crew, body: &Body, cell: [i32; 3]) -> bool {
    crew.aloft.perch.is_some()
        && (cell[0] - body.cell[0]).abs() + (cell[2] - body.cell[2]).abs() <= PERCH_REACH
        && cell[1] >= body.cell[1] - 1
}

/// The house goes up in order: work sits within a few layers of the lowest
/// unfinished one, waiting or not, and only after a long stall may the golem
/// look higher, instead of going from a floor to the roof and back.
pub(super) fn weigh_work(
    ctx: &mut Ctx,
    _projects: &mut Projects,
    job: &mut Job,
    body: &Body,
    round: &mut Round,
) -> Flow {
    let project = &round.project;
    let stalled = ctx.now > job.crew.pace.band_progress_at + BAND_PATIENCE;
    let floor = work_floor(job);
    // The band as it stood stays in view while the golem may still be held up
    // in it (see below).
    let ceiling = match (floor, stalled) {
        (Some(floor), false) => {
            let top = floor + LAYER_BAND;
            Some(job.crew.pace.band_top.map_or(top, |was| top.max(was)))
        }
        _ => None,
    };
    let Candidates {
        list: mut candidates,
        wants_items,
        waiting,
    } = gather(ctx, job, body, project, round.mode, ceiling);
    // Overgrowth in the way goes first, whatever its layer: it is why work
    // waits.
    let urgent = |task: &Task| task.urgent(&job.crew.scaffolding.urgent);
    if let Some(floor) = floor {
        // A lower block coming round again (a wait run out, a hedge laid bare)
        // drops the band below a golem on the roof: it finishes what is beside
        // it first.
        let was = job.crew.pace.band_top;
        let mut top = floor + LAYER_BAND;
        if let Some(was) = was {
            if top < was
                && candidates.iter().any(|(task, c)| {
                    !urgent(task)
                        && c[1] > top
                        && c[1] <= was
                        && manhattan(*c, body.cell) <= STAY_REACH
                })
            {
                top = was;
            }
        }
        job.crew.pace.band_top = Some(top);
        candidates.retain(|(task, c)| {
            urgent(task) || stalled || c[1] <= top || beside_the_perch(&job.crew, body, *c)
        });
    }
    // Nearest first, so a wall rises where the golem stands; only the nearest
    // are weighed at all, or the work in view is a whole course of the house.
    let late = |task: &Task| task.late(&job.design);
    candidates.sort_by_key(|(task, c)| (!urgent(task), manhattan(*c, body.cell)));
    candidates.truncate(WINDOW);
    candidates.sort_by_key(|(task, c)| (!urgent(task), late(task), manhattan(*c, body.cell)));
    round.candidates = candidates;
    round.wants_items = wants_items;
    round.waiting = waiting;
    Flow::Pass
}
