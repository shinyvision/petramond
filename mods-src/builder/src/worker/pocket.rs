mod graph;
mod grid;
mod rays;
mod recovery;
mod sight;
mod snapshot;
mod supports;

use crate::design::{Design, Plan as UnitPlan};
use crate::fx::{HashMap, HashSet};
use crate::geometry::offset;
use crate::host::prelude::*;
use crate::project::Project;
use crate::survey::{Known, Survey};
use crate::worker::tuning::access::{DIG_COST, MARGIN, RESTORE_COST};
use crate::worker::{Ctx, Job, Task};

use graph::{Cell, Graph, Work};
use grid::Bounds;
use recovery::Gate;
use snapshot::{Sample, Snapshot};

#[derive(Default)]
pub struct Plan {
    snapshot: Option<Snapshot>,
    graph: Option<Graph>,
    gates: Vec<Gate>,
    survey_revision: u64,
    world_revision: u64,
    ready: bool,
    sealed: Vec<usize>,
    held: HashSet<usize>,
    removable: HashSet<[i32; 3]>,
    sight: sight::Cache,
    prepared: Option<Prepared>,
    physical: HashSet<usize>,
}

struct Prepared {
    survey_revision: u64,
    world_revision: u64,
    cells: Vec<Cell>,
    raw: Vec<Cell>,
    cursor: usize,
    work: Vec<Work>,
    aims: HashMap<usize, sight::Aims>,
    graphed: bool,
    gate_cursor: usize,
}

impl Plan {
    pub fn route_failed(&mut self) {
        self.ready = false;
        self.graph = None;
        self.prepared = None;
    }

    pub fn stance(&self, task: Task, cell: [i32; 3]) -> bool {
        self.graph.as_ref().is_none_or(|g| match task {
            Task::Unit(i) => g.verified(i, cell),
            Task::Reopen(_) => g.bounds.index(cell).is_some_and(|i| g.ground.contains(&i)),
            _ => true,
        })
    }

    #[cfg(test)]
    pub fn unbudget_sight(&mut self, now: u64) {
        self.sight.unbudget(now);
    }

    pub fn changed(&mut self, cells: &[[i32; 3]], lost: bool) {
        self.sight.changed(cells, lost);
        if self
            .snapshot
            .as_mut()
            .is_some_and(|s| s.changed(cells, lost))
        {
            self.ready = false;
        }
    }

    pub fn score(&self) -> Option<usize> {
        self.ready
            .then(|| self.graph.as_ref().unwrap().sealed.len())
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

    pub fn can_clear(&self, cell: [i32; 3]) -> bool {
        self.ready && self.removable.contains(&cell)
    }
}

pub fn refresh(ctx: &mut Ctx, job: &mut Job, project: &Project) -> bool {
    let Some(survey) = job.survey.as_ref() else {
        return false;
    };
    let Some(mob) = job.crew.mob else {
        return false;
    };
    let min = std::array::from_fn(|a| job.design.min[a].min(project.home[a]) - MARGIN);
    let max = std::array::from_fn(|a| job.design.max[a].max(project.home[a]) + MARGIN);
    let bounds = Bounds::new(min, max);
    let plan = &mut job.access_plan;
    if plan.snapshot.as_ref().is_none_or(|s| s.bounds != bounds) {
        plan.snapshot = Some(Snapshot::new(bounds));
        plan.graph = None;
        plan.ready = false;
        plan.sight = sight::Cache::default();
        plan.prepared = None;
    }
    let snapshot = plan.snapshot.as_mut().unwrap();
    if !snapshot.read(ctx.now) {
        plan.ready = false;
        return false;
    }
    if survey.summary().is_none_or(|s| s.unchecked > 0) {
        plan.ready = false;
        return false;
    }
    if plan.graph.is_some()
        && plan.survey_revision == survey.revision()
        && plan.world_revision == snapshot.revision
    {
        plan.ready = true;
        return true;
    }
    if plan.prepared.as_ref().is_none_or(|p| {
        p.survey_revision != survey.revision() || p.world_revision != snapshot.revision
    }) {
        let (cells, raw) = classify(ctx, &job.design, survey, snapshot, project);
        plan.removable = raw
            .iter()
            .enumerate()
            .filter(|(_, c)| c.dig.is_some())
            .map(|(i, _)| bounds.cell(i))
            .collect();
        plan.prepared = Some(Prepared {
            survey_revision: survey.revision(),
            world_revision: snapshot.revision,
            cells,
            raw,
            cursor: 0,
            work: Vec::new(),
            aims: HashMap::default(),
            graphed: false,
            gate_cursor: 0,
        });
    }
    let prepared = plan.prepared.as_mut().unwrap();
    for i in prepared.cursor..job.design.units.len() {
        let unit = job.design.units[i];
        prepared.cursor = i;

        let (targets, pos, record) = match survey.known[i] {
            Known::Place(_) if !job.crew.built.contains(&i) => (
                job.design.cells(unit),
                unit.pos,
                Some(job.design.records[unit.record as usize].clone()),
            ),
            Known::Clear { at, .. } => (vec![at], at, None),
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
            plan.ready = false;
            return false;
        };
        prepared
            .aims
            .insert(i, plan.sight.aims(pos, record.is_some()));
        prepared.work.push(Work { unit: i, contacts });
    }
    prepared.cursor = job.design.units.len();
    if !prepared.graphed {
        let root = bounds.index(project.home).unwrap();
        let mut graph = Graph::new(
            bounds,
            prepared.cells.clone(),
            std::mem::take(&mut prepared.work),
            root,
        );
        graph.aims = std::mem::take(&mut prepared.aims);
        graph.sight_open = prepared.raw.iter().map(|c| c.open || c.sight).collect();
        let mut stances: Vec<_> = match super::route::region(ctx, project.home, false, &[]) {
            Some(Some(region)) => region.cells().collect(),
            Some(None) => Vec::new(),
            None => {
                prepared.work = graph.work;
                prepared.aims = graph.aims;
                plan.ready = false;
                return false;
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
        graph.ground = stances.iter().filter_map(|s| bounds.index(*s)).collect();
        graph.supports = supports::columns(&graph, &prepared.raw, &job.design.governed);
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
        let network = rays::Network::new(&graph, &prepared.raw, &targets, &stances);
        plan.physical.extend(network.below());
        let physical = graph
            .work
            .iter()
            .filter(|w| plan.physical.contains(&w.unit))
            .map(|w| {
                let contacts = w
                    .contacts
                    .iter()
                    .copied()
                    .filter(|c| {
                        (graph.ground.contains(c) || graph.supports.contains_key(c))
                            && !job
                                .crew
                                .deferrals
                                .blind(Task::Unit(w.unit), bounds.cell(*c))
                    })
                    .collect();
                (w.unit, contacts)
            })
            .collect();
        graph.physical(physical);
        plan.gates = recovery::plan(&graph);
        let sealed: Vec<_> = graph.sealed.iter().map(|w| graph.work[*w].unit).collect();
        for gate in network.openings(&sealed) {
            if let Some(existing) = plan.gates.iter_mut().find(|g| g.task == gate.task) {
                existing.serves = existing.serves.max(gate.serves);
            } else {
                plan.gates.push(gate);
            }
        }
        recovery::rank(&mut plan.gates);
        plan.graph = Some(graph);
        prepared.graphed = true;
    }
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
            plan.ready = false;
            return false;
        };
        gate.exposed = contacts.iter().any(|c| graph.ground.contains(c));
    }
    let recovering = !plan.sealed.is_empty();
    if recovering && graph.sealed.is_empty() {
        plan.held.extend(graph.work.iter().map(|w| w.unit));
    }
    plan.sealed = graph.sealed.iter().map(|i| graph.work[*i].unit).collect();
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
    plan.survey_revision = survey.revision();
    plan.world_revision = snapshot.revision;
    plan.ready = true;
    true
}

fn classify(
    ctx: &mut Ctx,
    design: &Design,
    survey: &Survey,
    snapshot: &Snapshot,
    project: &Project,
) -> (Vec<Cell>, Vec<Cell>) {
    let supplies: HashSet<_> = ctx.supplies.chain(project.table).into_iter().collect();
    let fragile: HashSet<_> = blocks_by_tag(crate::keys::FRAGILE_TAG)
        .into_iter()
        .collect();
    let clear: HashMap<_, _> = survey
        .known
        .iter()
        .enumerate()
        .filter_map(|(i, k)| match k {
            Known::Clear {
                at,
                holds_items: false,
                ..
            } => Some((*at, i)),
            _ => None,
        })
        .collect();
    let mut kinds = HashMap::default();
    let raw: Vec<Cell> = snapshot
        .samples
        .iter()
        .enumerate()
        .map(|(i, sample)| {
            let Sample::Block(block) = sample else {
                return Cell::default();
            };
            let (open, sight, dig) = *kinds.entry(*block).or_insert_with(|| {
                let Some(info) = ctx.caches.block(*block) else {
                    return (false, false, None);
                };
                let open =
                    (info.collision.is_empty() && info.fluid.is_none()) || super::swings(info);
                let dig = (!open
                    && info.fluid.is_none()
                    && !info.answers_use
                    && info.hardness >= 0.0
                    && *block != ctx.content.table
                    && !fragile.contains(block))
                .then(|| ((info.hardness * DIG_COST).ceil() as u32).saturating_add(1));
                (open, info.collision.is_empty() || super::swings(info), dig)
            });
            let pos = snapshot.bounds.cell(i);
            if open {
                return Cell {
                    open,
                    sight,
                    dig: None,
                    head: None,
                };
            }
            let supports_fragile =
                snapshot.bounds.neighbours(i).into_iter().flatten().any(
                    |n| matches!(snapshot.samples[n], Sample::Block(b) if fragile.contains(&b)),
                );
            let dig = dig
                .filter(|_| !supplies.contains(&pos) && !supports_fragile)
                .and_then(|cost| {
                    if let Some(&unit) = clear.get(&pos) {
                        return Some((Task::Unit(unit), cost));
                    }
                    if let Some(unit) = design.unit_at(pos) {
                        let u = design.units[unit];
                        let safe = matches!(survey.known[unit], Known::Satisfied)
                            && matches!(design.plan(u), UnitPlan::Build { .. })
                            && design.cells(u).len() == 1
                            && !design.fragile(u);
                        safe.then_some((Task::Reopen(unit), cost.saturating_add(RESTORE_COST)))
                    } else if !design.governed.contains(&pos) {
                        Some((Task::Breakout(pos), cost))
                    } else {
                        None
                    }
                });
            Cell {
                open,
                sight,
                dig,
                head: None,
            }
        })
        .collect();
    let cells = raw
        .iter()
        .enumerate()
        .map(|(i, foot)| {
            let Some(head) = snapshot.bounds.neighbours(i)[2].map(|n| raw[n]) else {
                return Cell::default();
            };
            let open = foot.open && head.open;
            let clearable = (foot.open || foot.dig.is_some()) && (head.open || head.dig.is_some());
            Cell {
                open,
                sight: foot.sight,
                dig: clearable.then_some(foot.dig).flatten(),
                head: clearable.then_some(head.dig).flatten(),
            }
        })
        .collect();
    (cells, raw)
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
    seal_check(ctx, job, unit, &body, &cells).is_some_and(|sealed| sealed.is_none())
}

fn seal_check(
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
    let after = super::route::region(ctx, home, false, walls)??;
    let perch = job
        .crew
        .mob
        .and_then(mob_info)
        .filter(|m| m.on_ground)
        .map(|m| super::standing_cell(m.pos));
    Some(graph.seals_ground(unit, body, walls, |cell| {
        after.contains(cell) || perch == Some(cell)
    }))
}

#[cfg(test)]
mod tests;
