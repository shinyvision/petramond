use std::collections::BTreeMap;

use crate::host::prelude::*;

use super::glazing::{only_glazing_left, ways_through};
use super::sealing::{only_cutting_work_left, swings_open};
use super::{trace, Flow, Mode, Round};
use crate::design::Design;
use crate::fx::HashSet;
use crate::geometry::{manhattan, offset, FACES};
use crate::project::Projects;
use crate::survey::{ItemKey, Known, Survey};
use crate::worker::crew::{Crew, Stamped};
use crate::worker::route::{self, Hubs};
use crate::worker::tuning::every::{HELD_EVERY, WAYS_EVERY};
use crate::worker::tuning::patience::{BAND_PATIENCE, GLAZE_PATIENCE};
use crate::worker::tuning::waits::OUT_OF_REACH;
use crate::worker::tuning::window::{LAYER_BAND, PERCH_REACH, SCAN, STAY_REACH, WINDOW};
use crate::worker::upkeep::open_block;
use crate::worker::Job;
use crate::worker::{cargo, support, Body, Ctx, Task};

#[derive(Default)]
pub(super) struct Candidates {
    pub(super) list: Vec<(Task, [i32; 3])>,
    pub(super) wants_items: bool,
    pub(super) waiting: bool,
}

impl Candidates {
    fn offer(&mut self, deferred: bool, task: Task, cell: [i32; 3]) {
        if deferred {
            self.waiting = true;
        } else {
            self.list.push((task, cell));
        }
    }
}

struct Judging<'a> {
    body: &'a Body,
    project: &'a crate::project::Project,
    closing: bool,
    glazing: bool,
    held: Option<&'a BTreeMap<ItemKey, u32>>,
    carried: &'a BTreeMap<ItemKey, u32>,
    ceiling: Option<i32>,
    scan: usize,
}

pub(super) fn gather(
    ctx: &mut Ctx,
    job: &mut Job,
    body: &Body,
    project: &crate::project::Project,
    mode: Mode,
    ceiling: Option<i32>,
) -> Candidates {
    let mut found = Candidates::default();
    let closing = only_cutting_work_left(ctx, job, None);
    let held = refresh_reads(ctx, job, body, project);
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
    let judging = Judging {
        body,
        project,
        closing,
        glazing,
        held: held.as_ref(),
        carried: &carried,
        ceiling,
        scan: if mode != Mode::Building { 0 } else { SCAN },
    };
    let open_cells = open_cells(&job.design, survey, crew, judging.scan);
    trace::held_ways_in(ctx, &job.design, survey, crew, closing);
    offer_units(ctx, &job.design, survey, crew, &judging, &mut found);
    offer_scaffolds(ctx, crew, body, project, &open_cells, &mut found);
    offer_reopens(&job.design, survey, crew, ctx.now, &mut found);
    offer_digs(ctx, crew, &mut found);
    offer_trims(&mut job.design, crew, ctx.now, &mut found);
    found
}

fn refresh_reads(
    ctx: &mut Ctx,
    job: &mut Job,
    body: &Body,
    project: &crate::project::Project,
) -> Option<BTreeMap<ItemKey, u32>> {
    let idle = ctx.now.saturating_sub(job.crew.pace.progress_at);
    if idle > GLAZE_PATIENCE {
        job.crew.glazing.under_way = true;
    }
    if job.crew.cargo.in_reach.stale(ctx.now, HELD_EVERY) {
        let stock = ctx.supplies.stock(project.table);
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
    job.crew.cargo.in_reach.value.clone()
}

fn open_cells(design: &Design, survey: &Survey, crew: &Crew, scan: usize) -> HashSet<[i32; 3]> {
    survey
        .known
        .iter()
        .enumerate()
        .skip(crew.pace.cursor)
        .take(scan)
        .filter(|(i, known)| matches!(known, Known::Place(_)) && !crew.built.contains(i))
        .map(|(i, _)| design.units[i].pos)
        .collect()
}

fn offer_units(
    ctx: &mut Ctx,
    design: &Design,
    survey: &Survey,
    crew: &mut Crew,
    judging: &Judging,
    found: &mut Candidates,
) {
    let bare = exposed(
        ctx,
        survey
            .known
            .iter()
            .skip(crew.pace.cursor)
            .take(judging.scan)
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
        .take(judging.scan)
    {
        if judging.ceiling.is_none() && open == WINDOW {
            break;
        }
        if !known.open() {
            continue;
        }
        let pos = design.units[i].pos;
        if judging.ceiling.is_some_and(|top| pos[1] > top)
            && !beside_the_perch(crew, judging.body, pos)
        {
            found.waiting = true;
            continue;
        }
        trace::passage(
            ctx,
            design,
            crew,
            i,
            known,
            judging.carried,
            judging.closing,
        );
        if crew.deferrals.deferred(Task::Unit(i), ctx.now) {
            found.waiting = true;
            continue;
        }
        let takes_a_place = match known {
            Known::Place(missing) => offer_placement(ctx, design, crew, judging, i, missing, found),
            Known::Clear { at, .. } if crew.scaffolding.cells.contains(at) => false,
            Known::Clear {
                at,
                holds_items: false,
                ..
            } => {
                if !bare.contains(at) {
                    buried.push((Task::Unit(i), *at));
                    false
                } else {
                    if !crew.aloft.perch.is_some_and(|p| p.holds(*at)) {
                        found.list.push((Task::Unit(i), *at));
                    }
                    true
                }
            }
            Known::Clear { .. } => {
                crew.note = "A container with items is in the way".into();
                true
            }
            Known::Unloaded | Known::Unchecked => {
                found.waiting = true;
                true
            }
            _ => true,
        };
        if takes_a_place {
            open += 1;
        }
    }
    if found.list.is_empty() {
        found.list.append(&mut buried);
    } else if !buried.is_empty() {
        found.waiting = true;
    }
}

fn offer_placement(
    ctx: &mut Ctx,
    design: &Design,
    crew: &mut Crew,
    judging: &Judging,
    i: usize,
    missing: &[ItemStackData],
    found: &mut Candidates,
) -> bool {
    if crew.built.contains(&i) {
        return false;
    }
    if design.passage(i) && !judging.closing && !swings_open(ctx.caches, design, i) {
        found.waiting = true;
        return false;
    }
    if design.glazing(i)
        && !judging.glazing
        && !crew.glazing.ahead.contains(&i)
        && crew.glazing.ways.value.contains(&i)
    {
        found.waiting = true;
        return false;
    }
    if judging
        .held
        .is_some_and(|held| !cargo::holds(held, missing))
    {
        found.wants_items = true;
        return false;
    }
    if !cargo::holds(judging.carried, missing) {
        found.wants_items = true;
        return true;
    }
    let pos = design.units[i].pos;
    if leans_on_scaffold(design, design.units[i], &judging.project.scaffolds) {
        found.waiting = true;
        return true;
    }
    if !crew.faces.floating.contains(&i) {
        found.list.push((Task::Unit(i), pos));
        return true;
    }
    match support::below(ctx, design, pos) {
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
    true
}

fn offer_scaffolds(
    ctx: &mut Ctx,
    crew: &mut Crew,
    body: &Body,
    project: &crate::project::Project,
    open_cells: &HashSet<[i32; 3]>,
    found: &mut Candidates,
) {
    let on_top = crew
        .aloft
        .perch
        .filter(|p| body.cell == p.top_cell() && crew.aloft.bridge.is_none());
    let walks_home = crew.aloft.perch.is_none() && !project.scaffolds.is_empty() && {
        let trail = crew.trail.clone();
        let hubs = Hubs::new(project.home, &trail);
        matches!(route::out(ctx, hubs, body.cell, &[]), Some(Route::Open))
    };
    if !walks_home && on_top.is_none() {
        return;
    }
    crew.scaffolding
        .urgent
        .retain(|c| project.scaffolds.contains(c));
    for cell in &project.scaffolds {
        if on_top.is_some_and(|p| p.on_column(*cell)) {
            continue;
        }
        if (cell[0] - body.cell[0]).abs() <= 1
            && (cell[2] - body.cell[2]).abs() <= 1
            && cell[1] < body.cell[1]
        {
            continue;
        }
        let task = Task::Scaffold(*cell);
        if crew.deferrals.deferred(task, ctx.now) {
            found.waiting = true;
        } else if crew.scaffolding.urgent.contains(cell) || !support::props_up(*cell, open_cells) {
            found.list.push((task, *cell));
        }
    }
}

fn offer_reopens(
    design: &Design,
    survey: &Survey,
    crew: &mut Crew,
    now: u64,
    found: &mut Candidates,
) {
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
            crew.deferrals.deferred(task, now),
            task,
            design.units[o].pos,
        );
    }
}

fn offer_digs(ctx: &mut Ctx, crew: &mut Crew, found: &mut Candidates) {
    if crew.access.digs.is_empty() {
        return;
    }
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

fn offer_trims(design: &mut Design, crew: &mut Crew, now: u64, found: &mut Candidates) {
    if crew.access.trims.is_empty() {
        return;
    }
    let overgrowth = design.overgrowth();
    let mut trims: Vec<[i32; 3]> = crew.access.trims.iter().copied().collect();
    trims.sort_unstable();
    let blocks = get_blocks(trims.clone());
    for (cell, block) in trims.into_iter().zip(blocks) {
        match block {
            Some(b) if overgrowth.contains(&b) => {
                let task = Task::Trim(cell);
                found.offer(crew.deferrals.deferred(task, now), task, cell);
            }
            Some(_) => {
                crew.access.trims.remove(&cell);
            }
            None => found.waiting = true,
        }
    }
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
        .zip(paged(beside.clone(), get_blocks))
        .filter(|(_, b)| b.is_some_and(|b| open_block(ctx, b)))
        .map(|(c, _)| *c)
        .collect();
    cells
        .into_iter()
        .filter(|c| FACES.iter().any(|f| open.contains(&offset(*c, *f))))
        .collect()
}

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

fn beside_the_perch(crew: &Crew, body: &Body, cell: [i32; 3]) -> bool {
    crew.aloft.perch.is_some()
        && (cell[0] - body.cell[0]).abs() + (cell[2] - body.cell[2]).abs() <= PERCH_REACH
        && cell[1] >= body.cell[1] - 1
}

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
    let urgent = |task: &Task| task.urgent(&job.crew.scaffolding.urgent);
    if let Some(floor) = floor {
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
    let late = |task: &Task| task.late(&job.design);
    candidates.sort_by_key(|(task, c)| (!urgent(task), manhattan(*c, body.cell)));
    candidates.truncate(WINDOW);
    candidates.sort_by_key(|(task, c)| (!urgent(task), late(task), manhattan(*c, body.cell)));
    round.candidates = candidates;
    round.wants_items = wants_items;
    round.waiting = waiting;
    Flow::Pass
}
