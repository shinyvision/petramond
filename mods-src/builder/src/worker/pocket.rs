mod classify;
mod graph;
mod grid;
mod rays;
mod recovery;
mod sight;
mod snapshot;
mod supports;

use crate::fx::{HashMap, HashSet};
use crate::geometry::offset;
use crate::host::prelude::*;
use crate::project::Project;
use crate::survey::Known;
use crate::worker::tuning::access::{CHANGED_MAX, MARGIN, REBUILD_EVERY};
use crate::worker::{Ctx, Job, Task};

use classify::{Classifier, Kind};
use graph::{Cell, Graph, Work};
use grid::Bounds;
use recovery::Gate;
use snapshot::{Sample, Snapshot};

#[derive(Default)]
pub struct Plan {
    snapshot: Option<Snapshot>,
    graph: Option<Graph>,
    gates: Vec<Gate>,
    /// Unit kinds the current graph was built from.
    kinds: Vec<Kind>,
    /// Counts the times `kinds` was taken.
    kinds_taken: u64,
    /// Raw cells whose sample changed after the current graph was built.
    since: HashSet<usize>,
    /// The graph matches the world and survey as last read.
    fresh: bool,
    built_at: u64,
    /// The graph is out of date, but with the changes since laid over it still proves placements safe.
    stale_ok: bool,
    /// An out-of-date graph could not clear a placement; only a current one can tell.
    wants_current: bool,
    /// Sealed units some planned opening serves; sealed work nothing can open does not hold up building.
    sealed: Vec<usize>,
    openable: HashSet<usize>,
    held: HashSet<usize>,
    removable: HashSet<[i32; 3]>,
    sight: sight::Cache,
    network: rays::Remembered,
    prepared: Option<Prepared>,
    physical: HashSet<usize>,
}

/// The classified site, patched as the world and survey change, and the sweep that turns it into a graph.
struct Prepared {
    survey_revision: u64,
    cells: Vec<Cell>,
    raw: Vec<Cell>,
    kinds: Vec<Kind>,
    /// The cells the design governs, by cell.
    governed: Option<Vec<bool>>,
    /// Units whose kind differs from the plan's, as of a survey revision and a taking of the plan's kinds.
    kind_diffs: Option<((u64, u64), Vec<usize>)>,
    /// Cells or unit kinds differ from the ones the current graph was built from.
    reshaped: bool,
    cursor: usize,
    work: Vec<Work>,
    aims: HashMap<usize, sight::Aims>,
    graphed: bool,
    gate_cursor: usize,
    done: bool,
}

impl Prepared {
    fn restart(&mut self) {
        self.cursor = 0;
        self.work.clear();
        self.aims.clear();
        self.graphed = false;
        self.gate_cursor = 0;
        self.done = false;
    }
}

enum Progress {
    Fresh,
    Behind,
    /// The sweep is done and the graph is out of date; building a new one is the next step.
    Due,
    Unread,
}

impl Plan {
    pub fn route_failed(&mut self) {
        self.fresh = false;
        self.stale_ok = false;
        self.graph = None;
        self.since.clear();
        if let Some(prepared) = &mut self.prepared {
            prepared.restart();
            prepared.reshaped = true;
        }
    }

    /// The refresh state, for traces.
    pub fn describe(&self) -> String {
        format!(
            "graph {} fresh {} stale_ok {} wants_current {} built_at {} since {} prepared {} cursor {} sealed {}",
            self.graph.is_some(),
            self.fresh,
            self.stale_ok,
            self.wants_current,
            self.built_at,
            self.since.len(),
            self.prepared.is_some(),
            self.prepared.as_ref().map_or(0, |p| p.cursor),
            self.sealed.len(),
        )
    }

    pub fn stance(&self, task: Task, cell: [i32; 3]) -> bool {
        self.graph.as_ref().is_none_or(|g| match task {
            Task::Unit(i) => g.verified(i, cell),
            Task::Reopen(_) => g.bounds.index(cell).is_some_and(|i| g.ground[i]),
            _ => true,
        })
    }

    #[cfg(test)]
    pub fn unbudget_sight(&mut self, now: u64) {
        self.sight.unbudget(now);
    }

    #[cfg(test)]
    pub fn starve_sight(&mut self, now: u64) {
        self.sight.budget(now, 0);
    }

    #[cfg(test)]
    pub fn fresh(&self) -> bool {
        self.fresh
    }

    #[cfg(test)]
    pub fn want_current(&mut self) {
        self.wants_current = true;
    }

    pub fn changed(&mut self, cells: &[[i32; 3]], lost: bool) {
        self.sight.changed(cells, lost);
        if self
            .snapshot
            .as_mut()
            .is_some_and(|s| s.changed(cells, lost))
        {
            self.fresh = false;
            self.stale_ok = false;
        }
    }

    /// Sealed work across the site; an out-of-date graph answers only while it proves nothing is sealed.
    pub fn score(&self) -> Option<usize> {
        self.graph.as_ref()?;
        if self.fresh {
            Some(self.sealed.len())
        } else {
            self.stale_ok.then_some(0)
        }
    }

    /// A refresh has work left that ticks can spend their budget on.
    pub fn behind(&self) -> bool {
        self.prepared.is_some() && !self.fresh
    }

    pub fn opens(&self, task: Task) -> bool {
        self.score().is_some_and(|n| n > 0) && self.gates.iter().any(|g| g.task == task)
    }

    pub fn benefit(&self, task: Task) -> u32 {
        self.gates
            .iter()
            .find(|g| g.task == task)
            .map_or(0, |g| g.serves)
    }

    pub fn digging(&self) -> impl Iterator<Item = [i32; 3]> + '_ {
        self.gates.iter().map(|g| g.cell)
    }

    pub fn candidates(&self) -> Vec<(Task, [i32; 3])> {
        self.gates
            .iter()
            .filter(|g| g.exposed)
            .map(|g| (g.task, g.cell))
            .collect()
    }

    pub fn sealed_units(&self) -> Vec<usize> {
        self.sealed.clone()
    }

    /// Stops counting `units` as sealed work holding up the build.
    pub fn set_aside(&mut self, units: &HashSet<usize>) {
        self.sealed.retain(|u| !units.contains(u));
    }

    pub fn can_clear(&self, cell: [i32; 3]) -> bool {
        self.fresh && self.removable.contains(&cell)
    }
}

/// Brings the access graph up to date as far as this tick's budgets allow; true when it may be relied on.
///
/// A whole-site graph costs far more than one tick's share, so while the last one still proves
/// building safe it stands in until it is old or many cells have changed under it.
pub fn refresh(ctx: &mut Ctx, job: &mut Job, project: &Project) -> bool {
    let mut progress = rebuild(ctx, job, project, false);
    let mut usable = None;
    if matches!(progress, Progress::Due) {
        let plan = &job.access_plan;
        let young = plan.graph.is_some()
            && !plan.wants_current
            && ctx.now < plan.built_at + REBUILD_EVERY
            && plan.since.len() < CHANGED_MAX;
        usable = Some(young && stale_ok(ctx, job));
        if usable == Some(false) {
            progress = rebuild(ctx, job, project, true);
            usable = None;
        }
    }
    job.access_plan.fresh = matches!(progress, Progress::Fresh);
    job.access_plan.stale_ok = !job.access_plan.fresh
        && !matches!(progress, Progress::Unread)
        && usable.unwrap_or_else(|| stale_ok(ctx, job));
    job.access_plan.fresh || job.access_plan.stale_ok
}

fn rebuild(ctx: &mut Ctx, job: &mut Job, project: &Project, build_now: bool) -> Progress {
    let Some(survey) = job.survey.as_ref() else {
        return Progress::Unread;
    };
    let Some(mob) = job.crew.mob else {
        return Progress::Unread;
    };
    let min = std::array::from_fn(|a| job.design.min[a].min(project.home[a]) - MARGIN);
    let max = std::array::from_fn(|a| job.design.max[a].max(project.home[a]) + MARGIN);
    let bounds = Bounds::new(min, max);
    let plan = &mut job.access_plan;
    if plan.snapshot.as_ref().is_none_or(|s| s.bounds != bounds) {
        plan.snapshot = Some(Snapshot::new(bounds));
        plan.graph = None;
        plan.since.clear();
        plan.sight = sight::Cache::default();
        plan.prepared = None;
    }
    let snapshot = plan.snapshot.as_mut().unwrap();
    if !snapshot.read(ctx.now) {
        return Progress::Unread;
    }
    if survey.summary().is_none_or(|s| s.unchecked > 0) {
        return Progress::Unread;
    }
    let changes = snapshot.take_changes();
    if plan.graph.is_some() {
        plan.since.extend(changes.iter().copied());
    }
    let snapshot = plan.snapshot.as_ref().unwrap();
    let kind_of = |i: usize, k: &Known| {
        Kind::of(
            k,
            matches!(k, Known::Place(_)) && job.crew.built.contains(&i),
        )
    };
    match plan.prepared.as_mut() {
        None => {
            let mut classifier = Classifier::new(ctx, &job.design, survey, snapshot, project);
            let (cells, raw) = classifier.all(ctx);
            plan.removable = raw
                .iter()
                .enumerate()
                .filter(|(_, c)| c.dig.is_some())
                .map(|(i, _)| bounds.cell(i))
                .collect();
            plan.prepared = Some(Prepared {
                survey_revision: survey.revision(),
                cells,
                raw,
                kinds: survey
                    .known
                    .iter()
                    .enumerate()
                    .map(|(i, k)| kind_of(i, k))
                    .collect(),
                kind_diffs: None,
                governed: None,
                reshaped: true,
                cursor: 0,
                work: Vec::new(),
                aims: HashMap::default(),
                graphed: false,
                gate_cursor: 0,
                done: false,
            });
        }
        Some(prepared) => {
            let mut touched = changes.clone();
            let mut moved = false;
            if prepared.survey_revision != survey.revision() {
                prepared.survey_revision = survey.revision();
                for (i, known) in survey.known.iter().enumerate() {
                    let kind = kind_of(i, known);
                    let was = std::mem::replace(&mut prepared.kinds[i], kind);
                    if was == kind {
                        continue;
                    }
                    moved = true;
                    // Clearing targets and reopenable blocks are classified by unit status.
                    let unit = job.design.units[i];
                    for k in [was, kind] {
                        if let Kind::Clear { at, .. } = k {
                            touched.extend(bounds.index(at));
                        }
                    }
                    touched.extend(bounds.index(unit.pos));
                }
            }
            if !touched.is_empty() {
                let mut classifier = Classifier::new(ctx, &job.design, survey, snapshot, project);
                prepared.reshaped |= classifier.patch(
                    ctx,
                    &mut prepared.cells,
                    &mut prepared.raw,
                    &touched,
                    &mut plan.removable,
                );
            }
            if !changes.is_empty() || moved {
                prepared.restart();
            }
            if prepared.done {
                return Progress::Fresh;
            }
        }
    }
    let prepared = plan.prepared.as_mut().unwrap();
    for i in prepared.cursor..job.design.units.len() {
        let unit = job.design.units[i];
        prepared.cursor = i;
        let known = match prepared.kinds[i] {
            Kind::Place => plan.sight.settled(unit.pos, true),
            Kind::Clear { at, .. } => plan.sight.settled(at, false),
            _ => continue,
        };
        if let Some((contacts, aims)) = known {
            prepared.aims.insert(i, aims);
            prepared.work.push(Work { unit: i, contacts });
            continue;
        }
        let (targets, pos, record) = match prepared.kinds[i] {
            Kind::Place => (
                job.design.cells(unit),
                unit.pos,
                Some(job.design.records[unit.record as usize].clone()),
            ),
            Kind::Clear { at, .. } => (vec![at], at, None),
            _ => continue,
        };
        let Some(contacts) = plan.sight.contacts(
            ctx.now,
            bounds,
            &prepared.cells,
            &prepared.raw,
            EntityRef::Entity(mob),
            &targets,
            pos,
            record.clone(),
        ) else {
            return Progress::Behind;
        };
        prepared
            .aims
            .insert(i, plan.sight.aims(pos, record.is_some()));
        prepared.work.push(Work { unit: i, contacts });
    }
    prepared.cursor = job.design.units.len();
    if !prepared.graphed {
        let unchanged = !prepared.reshaped
            && plan
                .graph
                .as_ref()
                .is_some_and(|g| same_work(g, &prepared.work, &prepared.aims));
        if unchanged {
            prepared.work.clear();
            prepared.aims.clear();
        } else {
            if !build_now {
                return Progress::Due;
            }
            let Some(graph) = build(ctx, job, project, bounds, mob) else {
                return Progress::Behind;
            };
            let plan = &mut job.access_plan;
            plan.graph = Some(graph);
        }
        let plan = &mut job.access_plan;
        let prepared = plan.prepared.as_mut().unwrap();
        plan.kinds = prepared.kinds.clone();
        plan.kinds_taken += 1;
        plan.since.clear();
        plan.built_at = ctx.now;
        plan.wants_current = false;
        prepared.reshaped = false;
        prepared.graphed = true;
        prepared.gate_cursor = 0;
    }
    let plan = &mut job.access_plan;
    let prepared = plan.prepared.as_mut().unwrap();
    let graph = plan.graph.as_ref().unwrap();
    for g in prepared.gate_cursor..plan.gates.len() {
        let gate = &mut plan.gates[g];
        prepared.gate_cursor = g;
        let Some(contacts) = plan.sight.contacts(
            ctx.now,
            bounds,
            &graph.cells,
            &prepared.raw,
            EntityRef::Entity(mob),
            &[gate.cell],
            gate.cell,
            None,
        ) else {
            return Progress::Behind;
        };
        gate.exposed = contacts.iter().any(|c| graph.ground[*c]);
    }
    prepared.gate_cursor = plan.gates.len();
    prepared.done = true;
    settled(job);
    Progress::Fresh
}

fn same_work(graph: &Graph, work: &[Work], aims: &HashMap<usize, sight::Aims>) -> bool {
    graph.work.len() == work.len()
        && graph.work.iter().zip(work).all(|(old, new)| {
            old.unit == new.unit
                && (std::sync::Arc::ptr_eq(&old.contacts, &new.contacts)
                    || old.contacts == new.contacts)
                && match (graph.aims.get(&old.unit), aims.get(&new.unit)) {
                    (Some(a), Some(b)) => std::sync::Arc::ptr_eq(a, b) || a == b,
                    (a, b) => a.is_none() && b.is_none(),
                }
        })
}

/// Builds the graph from a finished sweep; `None` while the native walking region is still being read.
fn build(
    ctx: &mut Ctx,
    job: &mut Job,
    project: &Project,
    bounds: Bounds,
    mob: u64,
) -> Option<Graph> {
    let survey = job.survey.as_ref().unwrap();
    let plan = &mut job.access_plan;
    let snapshot = plan.snapshot.as_ref().unwrap();
    let prepared = plan.prepared.as_mut().unwrap();
    let root = bounds.index(project.home).unwrap();
    let mut graph = Graph::new(
        bounds,
        prepared.cells.clone(),
        std::mem::take(&mut prepared.work),
        root,
    );
    graph.aims = std::mem::take(&mut prepared.aims);
    if let Some(old) = plan.graph.as_mut() {
        graph.faceless_judged = std::mem::take(&mut old.faceless_judged);
    }
    graph.sight_open = prepared.raw.iter().map(|c| c.open || c.sight).collect();
    graph.raw_open = prepared.raw.iter().map(|c| c.open).collect();
    let mut stances: Vec<_> = match super::route::both_ways(ctx, project.home, &[]) {
        Some(Some((out, back))) => out.cells().filter(|c| back.contains(*c)).collect(),
        Some(None) => Vec::new(),
        None => {
            prepared.work = graph.work;
            prepared.aims = graph.aims;
            return None;
        }
    };
    if let Some(info) = mob_info(mob).filter(|m| m.on_ground) {
        stances.push(super::standing_cell(info.pos));
    }
    stances.retain(|s| {
        bounds.index(*s).is_some_and(|i| graph.cells[i].open)
            && bounds.index(offset(*s, [0, -1, 0])).is_some_and(|i| {
                matches!(snapshot.samples[i], Sample::Block(_)) && !prepared.raw[i].sight
            })
            && !job.crew.access.failed_stances.holds(s, ctx.now)
    });
    for i in stances.iter().filter_map(|s| bounds.index(*s)) {
        graph.ground[i] = true;
    }
    let shut = prepared
        .governed
        .get_or_insert_with(|| supports::governed_cells(bounds, &job.design.governed));
    graph.supports = supports::columns(&graph, &prepared.raw, shut);
    let targets: Vec<_> = graph
        .work
        .iter()
        .map(|w| {
            let i = w.unit;
            let cell = match survey.known[i] {
                Known::Clear { at, .. } => at,
                _ => job.design.units[i].pos,
            };
            (i, cell)
        })
        .collect();
    let network =
        rays::Network::remembering(&graph, &prepared.raw, &targets, &stances, &mut plan.network);
    plan.physical.extend(network.below());
    let blind = job.crew.deferrals.blind_units();
    let physical = graph
        .work
        .iter()
        .filter(|w| plan.physical.contains(&w.unit))
        .map(|w| {
            let blind = blind.get(&w.unit);
            let contacts = w
                .contacts
                .iter()
                .copied()
                .filter(|c| {
                    (graph.ground[*c] || graph.supports.contains(*c))
                        && blind.is_none_or(|stances| !stances.contains(&bounds.cell(*c)))
                })
                .collect();
            (w.unit, contacts)
        })
        .collect();
    graph.physical(physical);
    let (gates, mut openable) = recovery::plan(&graph);
    plan.gates = gates;
    let sealed: Vec<_> = graph.sealed.iter().map(|w| graph.work[*w].unit).collect();
    openable.extend(sealed.iter().copied().filter(|u| network.opens(*u)));
    plan.openable = openable;
    for gate in network.openings(&sealed) {
        if let Some(existing) = plan.gates.iter_mut().find(|g| g.task == gate.task) {
            existing.serves = existing.serves.max(gate.serves);
        } else {
            plan.gates.push(gate);
        }
    }
    recovery::rank(&mut plan.gates);
    Some(graph)
}

/// Bookkeeping once a refresh has caught up with the world.
fn settled(job: &mut Job) {
    let survey = job.survey.as_ref().unwrap();
    let plan = &mut job.access_plan;
    let graph = plan.graph.as_ref().unwrap();
    let recovering = !plan.sealed.is_empty();
    let sealed: Vec<usize> = graph
        .sealed
        .iter()
        .map(|i| graph.work[*i].unit)
        .filter(|u| plan.openable.contains(u) && !job.crew.access.abandoned.contains(u))
        .collect();
    if recovering && sealed.is_empty() {
        plan.held.extend(graph.work.iter().map(|w| w.unit));
    }
    plan.sealed = sealed;
    if recovering || !plan.sealed.is_empty() {
        if let Some((work, _)) = &mut job.crew.access.way_in {
            work.retain(|cell| {
                plan.sealed
                    .iter()
                    .any(|i| job.design.units[*i].pos == *cell)
            });
            if work.is_empty() {
                job.crew.access.way_in = None;
            }
        }
    }
    for unit in plan.held.drain() {
        job.crew.deferrals.deferred.lift(&Task::Unit(unit));
    }
    trace!(
        "TRACE access: {} sealed units, {} planned openings {:?}",
        graph.sealed.len(),
        plan.gates.len(),
        graph
            .sealed
            .iter()
            .map(|w| {
                let i = graph.work[*w].unit;
                (i, job.design.units[i].pos, &survey.known[i])
            })
            .collect::<Vec<_>>()
    );
    let active = graph
        .work
        .iter()
        .map(|w| match survey.known[w.unit] {
            Known::Clear { at, .. } => (at, false),
            _ => (job.design.units[w.unit].pos, true),
        })
        .chain(plan.gates.iter().map(|g| (g.cell, false)))
        .collect();
    plan.sight.retain(&active);
}

/// Whether the last graph, with every cell closed and unit finished since laid over it, still proves nothing sealed.
fn stale_ok(ctx: &mut Ctx, job: &mut Job) -> bool {
    let plan = &mut job.access_plan;
    let (Some(graph), Some(prepared)) = (plan.graph.as_mut(), plan.prepared.as_mut()) else {
        return false;
    };
    if !plan.sealed.is_empty() {
        return false;
    }
    let key = (prepared.survey_revision, plan.kinds_taken);
    if prepared.kind_diffs.as_ref().is_none_or(|(k, _)| *k != key) {
        let diffs = plan
            .kinds
            .iter()
            .zip(&prepared.kinds)
            .enumerate()
            .filter(|(_, (then, now))| then != now)
            .map(|(i, _)| i)
            .collect();
        prepared.kind_diffs = Some((key, diffs));
    }
    let mut done = HashSet::default();
    for &i in &prepared.kind_diffs.as_ref().unwrap().1 {
        let (then, now) = (plan.kinds[i], prepared.kinds[i]);
        // The worker's own scaffold standing in a room it climbs through comes down with the
        // climb; the next scheduled rebuild picks it up if it stays.
        let scaffold =
            matches!(now, Kind::Clear { at, .. } if job.crew.scaffolding.cells.contains(&at));
        if now.work() && !scaffold {
            return false;
        }
        if then.work() {
            done.insert(i);
        }
    }
    let mut closed = Vec::new();
    let mut walls = Vec::new();
    for &i in &plan.since {
        let now = prepared.raw[i];
        let cell = graph.bounds.cell(i);
        if graph.raw_open[i] && !now.open {
            closed.push(cell);
        }
        if graph.sight_open[i] && !(now.open || now.sight) {
            walls.push(cell);
        }
    }
    graph.since(&closed, walls, done);
    check(ctx, job, None, &[], &[]) == Some(None)
}

fn body_walls(cells: &[[i32; 3]]) -> Vec<[i32; 3]> {
    let mut blocked = cells.to_vec();
    blocked.extend(cells.iter().map(|c| offset(*c, [0, -1, 0])));
    blocked.sort_unstable();
    blocked.dedup();
    blocked
}

pub fn seals(ctx: &mut Ctx, job: &mut Job, i: usize) -> Option<Option<usize>> {
    job.access_plan.score()?;
    let unit = job.design.units[i];
    let info = ctx
        .caches
        .block_named(&job.design.records[unit.record as usize].block)?;
    if info.collision.is_empty() || super::swings(info) {
        return Some(None);
    }
    let cells = job.design.cells(job.design.units[i]);
    let cells = super::plan::as_walls(ctx, job, &cells);
    let body = body_walls(&cells);
    let answer = seal_check(ctx, job, Some(i), &body, &cells);
    if answer.is_some_and(|a| a.is_some()) {
        job.access_plan.held.insert(i);
    }
    answer
}

pub fn placement(ctx: &mut Ctx, job: &mut Job, cell: [i32; 3], record: &CellRecord) -> bool {
    if job.access_plan.score().is_none() {
        return false;
    }
    let unit = job
        .design
        .unit_at(cell)
        .filter(|i| job.design.records[job.design.units[*i].record as usize] == *record);
    if unit.is_some() && job.access_plan.score().is_none_or(|n| n > 0) {
        return false;
    }
    let Some(info) = ctx.caches.block_named(&record.block) else {
        return false;
    };
    if info.collision.is_empty() || super::swings(info) {
        return true;
    }
    let cells = unit.map_or_else(|| vec![cell], |i| job.design.cells(job.design.units[i]));
    let cells = super::plan::as_walls(ctx, job, &cells);
    let body = body_walls(&cells);
    match (seal_check(ctx, job, unit, &body, &cells), unit) {
        // Of two blocks sealing each other, the lower goes in.
        (Some(Some(sealed)), Some(i)) if i < sealed => seals(ctx, job, sealed) == Some(Some(i)),
        (answer, _) => answer.is_some_and(|sealed| sealed.is_none()),
    }
}

/// An out-of-date graph cannot tell a seal from a change it has not seen, so it reports one as unknown.
fn seal_check(
    ctx: &mut Ctx,
    job: &mut Job,
    unit: Option<usize>,
    body: &[[i32; 3]],
    walls: &[[i32; 3]],
) -> Option<Option<usize>> {
    let answer = check(ctx, job, unit, body, walls)?;
    if job.access_plan.fresh || answer.is_none() {
        return Some(answer);
    }
    job.access_plan.wants_current = true;
    None
}

fn check(
    ctx: &mut Ctx,
    job: &mut Job,
    unit: Option<usize>,
    body: &[[i32; 3]],
    walls: &[[i32; 3]],
) -> Option<Option<usize>> {
    let graph = job.access_plan.graph.as_mut()?;
    if let Some(sealed) = graph.seals_sight(unit, body, walls) {
        return Some(Some(sealed));
    }
    if !graph.needs_ground() {
        return Some(None);
    }
    let home = graph.bounds.cell(graph.root);
    let perch = job
        .crew
        .mob
        .and_then(mob_info)
        .filter(|m| m.on_ground)
        .map(|m| super::standing_cell(m.pos));
    let ((out0, back0), (out, back)) = super::route::both_ways_and_without(ctx, home, walls)??;
    let key = (
        unit,
        body.to_vec(),
        walls.to_vec(),
        perch,
        [out0.stamp(), back0.stamp(), out.stamp(), back.stamp()],
    );
    Some(graph.seals_ground_remembered(
        key,
        |cell| (out.contains(cell) && back.contains(cell)) || perch == Some(cell),
        |cell| (out0.contains(cell) && back0.contains(cell)) || perch == Some(cell),
    ))
}

#[cfg(test)]
mod tests;
