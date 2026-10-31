use crate::design::{Design, Plan as UnitPlan};
use crate::fx::{HashMap, HashSet};
use crate::host::prelude::*;
use crate::project::Project;
use crate::survey::{Known, Survey};
use crate::worker::tuning::access::{DIG_COST, RESTORE_COST};
use crate::worker::{Ctx, Task};

use super::graph::Cell;
use super::snapshot::{Sample, Snapshot};

/// What a unit's survey status means to the access graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Satisfied,
    Place,
    Built,
    Clear { at: [i32; 3], free: bool },
    Unknown,
}

impl Kind {
    pub fn of(known: &Known, built: bool) -> Self {
        match known {
            Known::Satisfied => Kind::Satisfied,
            Known::Place(_) if built => Kind::Built,
            Known::Place(_) => Kind::Place,
            Known::Clear {
                at, holds_items, ..
            } => Kind::Clear {
                at: *at,
                free: !holds_items,
            },
            _ => Kind::Unknown,
        }
    }

    /// Unfinished work the graph must keep accessible.
    pub fn work(self) -> bool {
        matches!(self, Kind::Place | Kind::Clear { .. })
    }
}

pub(super) struct Classifier<'a> {
    design: &'a Design,
    survey: &'a Survey,
    snapshot: &'a Snapshot,
    supplies: HashSet<[i32; 3]>,
    fragile: HashSet<BlockId>,
    clear: HashMap<[i32; 3], usize>,
    kinds: HashMap<BlockId, (bool, bool, Option<u32>)>,
}

impl<'a> Classifier<'a> {
    pub fn new(
        ctx: &mut Ctx,
        design: &'a Design,
        survey: &'a Survey,
        snapshot: &'a Snapshot,
        project: &Project,
    ) -> Self {
        Self {
            design,
            survey,
            snapshot,
            supplies: ctx.supplies.chain(project.table).into_iter().collect(),
            fragile: blocks_by_tag(crate::keys::FRAGILE_TAG)
                .into_iter()
                .collect(),
            clear: survey
                .known
                .iter()
                .enumerate()
                .filter_map(|(i, k)| match k {
                    Known::Clear {
                        at,
                        holds_items: false,
                        ..
                    } if !crate::worker::plan::regrows(ctx, design, i, k) => Some((*at, i)),
                    _ => None,
                })
                .collect(),
            kinds: HashMap::default(),
        }
    }

    pub fn all(&mut self, ctx: &mut Ctx) -> (Vec<Cell>, Vec<Cell>) {
        let raw: Vec<Cell> = (0..self.snapshot.samples.len())
            .map(|i| self.raw(ctx, i))
            .collect();
        let cells = (0..raw.len()).map(|i| self.pair(&raw, i)).collect();
        (cells, raw)
    }

    /// Reclassifies the cells around `changed` in place; true when any cell differs.
    pub fn patch(
        &mut self,
        ctx: &mut Ctx,
        cells: &mut [Cell],
        raw: &mut [Cell],
        changed: &[usize],
        removable: &mut HashSet<[i32; 3]>,
    ) -> bool {
        let bounds = self.snapshot.bounds;
        let mut feet: Vec<usize> = changed
            .iter()
            .flat_map(|&i| std::iter::once(Some(i)).chain(bounds.neighbours(i)))
            .flatten()
            .collect();
        feet.sort_unstable();
        feet.dedup();
        let mut differs = false;
        for &i in &feet {
            let cell = self.raw(ctx, i);
            if cell != raw[i] {
                differs = true;
                raw[i] = cell;
                let pos = bounds.cell(i);
                if cell.dig.is_some() {
                    removable.insert(pos);
                } else {
                    removable.remove(&pos);
                }
            }
        }
        let mut pairs: Vec<usize> = feet
            .iter()
            .flat_map(|&i| [Some(i), bounds.neighbours(i)[3]])
            .flatten()
            .collect();
        pairs.sort_unstable();
        pairs.dedup();
        for i in pairs {
            let cell = self.pair(raw, i);
            if cell != cells[i] {
                differs = true;
                cells[i] = cell;
            }
        }
        differs
    }

    fn raw(&mut self, ctx: &mut Ctx, i: usize) -> Cell {
        let snapshot = self.snapshot;
        let Sample::Block(block) = snapshot.samples[i] else {
            return Cell::default();
        };
        let (open, sight, dig) = *self.kinds.entry(block).or_insert_with(|| {
            let Some(info) = ctx.caches.block(block) else {
                return (false, false, None);
            };
            let open =
                (info.collision.is_empty() && info.fluid.is_none()) || crate::worker::swings(info);
            let dig = (!open
                && info.fluid.is_none()
                && !info.answers_use
                && info.hardness >= 0.0
                && block != ctx.content.table
                && !self.fragile.contains(&block))
            .then(|| ((info.hardness * DIG_COST).ceil() as u32).saturating_add(1));
            (
                open,
                info.collision.is_empty() || crate::worker::swings(info),
                dig,
            )
        });
        if open {
            return Cell {
                open,
                sight,
                dig: None,
                head: None,
            };
        }
        let pos = snapshot.bounds.cell(i);
        let supports_fragile =
            snapshot.bounds.neighbours(i).into_iter().flatten().any(
                |n| matches!(snapshot.samples[n], Sample::Block(b) if self.fragile.contains(&b)),
            );
        let design = self.design;
        let survey = self.survey;
        let dig = dig
            .filter(|_| !self.supplies.contains(&pos) && !supports_fragile)
            .and_then(|cost| {
                if let Some(&unit) = self.clear.get(&pos) {
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
    }

    fn pair(&self, raw: &[Cell], i: usize) -> Cell {
        let foot = raw[i];
        let Some(head) = self.snapshot.bounds.neighbours(i)[2].map(|n| raw[n]) else {
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
    }
}
