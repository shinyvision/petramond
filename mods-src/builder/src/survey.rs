use std::collections::{BTreeMap, VecDeque};

use crate::fx::HashMap;

use crate::host::prelude::*;

use crate::design::{Design, Plan};

pub type ItemKey = (String, Vec<(String, Vec<u8>)>);

pub fn key_of(stack: &ItemStackData) -> ItemKey {
    (stack.item.clone(), stack.data.clone())
}

#[derive(Clone, Debug, PartialEq)]
pub enum Known {
    Unchecked,
    Unloaded,
    Satisfied,
    Place(Vec<ItemStackData>),
    Clear {
        at: [i32; 3],
        block: BlockId,
        holds_items: bool,
    },
    Pending,
    Unsupported(String),
}

impl Known {
    fn of(status: RecordStatus) -> Self {
        match status {
            RecordStatus::Unloaded => Known::Unloaded,
            RecordStatus::Satisfied => Known::Satisfied,
            RecordStatus::Place { missing } => Known::Place(missing),
            RecordStatus::Clear {
                at,
                block,
                holds_items,
                ..
            } => Known::Clear {
                at,
                block,
                holds_items,
            },
            RecordStatus::Pending { .. } => Known::Pending,
            RecordStatus::Unsupported { reason } => Known::Unsupported(reason),
        }
    }

    pub fn open(&self) -> bool {
        !matches!(self, Known::Satisfied)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    pub bill: BTreeMap<ItemKey, u32>,
    pub clear: u32,
    pub clear_blocks: crate::fx::HashMap<BlockId, u32>,
    pub guarded: u32,
    pub unchecked: u32,
    pub unsupported: u32,
    pub first_unsupported: String,
    pub open: u32,
}

impl Summary {
    fn count(&mut self, known: &Known, cost: &[ItemStackData], add: bool) {
        fn step(n: &mut u32, add: bool) {
            *n = if add { *n + 1 } else { n.saturating_sub(1) };
        }
        let bill = |stacks: &[ItemStackData], bill: &mut BTreeMap<ItemKey, u32>| {
            for stack in stacks {
                let key = key_of(stack);
                let count = u32::from(stack.count);
                if add {
                    *bill.entry(key).or_default() += count;
                } else if let Some(n) = bill.get_mut(&key) {
                    *n = n.saturating_sub(count);
                    if *n == 0 {
                        bill.remove(&key);
                    }
                }
            }
        };
        match known {
            Known::Satisfied => return,
            Known::Unchecked | Known::Unloaded => step(&mut self.unchecked, add),
            Known::Place(missing) => bill(missing, &mut self.bill),
            Known::Clear {
                holds_items, block, ..
            } => {
                step(&mut self.clear, add);
                let of_block = self.clear_blocks.entry(*block).or_default();
                step(of_block, add);
                if *of_block == 0 {
                    self.clear_blocks.remove(block);
                }
                if *holds_items {
                    step(&mut self.guarded, add);
                }
                bill(cost, &mut self.bill);
            }
            Known::Pending => {}
            Known::Unsupported(reason) => {
                if add && self.unsupported == 0 {
                    self.first_unsupported = reason.clone();
                }
                step(&mut self.unsupported, add);
            }
        }
        step(&mut self.open, add);
    }
}

const SWEEP: usize = 32;
const SWEEP_CADENCE: Cadence = Cadence::every(8);
const RETRY: Cadence = Cadence::every(10);

pub struct Survey {
    pub known: Vec<Known>,
    totals: Summary,
    first_pass: usize,
    queue: VecDeque<usize>,
    queued: Vec<bool>,
    waiting: Vec<usize>,
    is_waiting: Vec<bool>,
    by_cell: HashMap<[i32; 3], Vec<u32>>,
    stepped: u64,
    sweep: usize,
}

impl Survey {
    pub fn new(design: &Design) -> Self {
        let known: Vec<Known> = design
            .units
            .iter()
            .map(|u| match design.plan(*u) {
                Plan::Unsupported(reason) => Known::Unsupported(reason.clone()),
                _ => Known::Unchecked,
            })
            .collect();
        let mut totals = Summary::default();
        let mut by_cell: HashMap<[i32; 3], Vec<u32>> = HashMap::default();
        let mut queue = VecDeque::new();
        let mut queued = vec![false; known.len()];
        for (i, unit) in design.units.iter().enumerate() {
            totals.count(&known[i], design.cost(*unit), true);
            if matches!(known[i], Known::Unsupported(_)) {
                continue;
            }
            for cell in design.cells(*unit) {
                by_cell.entry(cell).or_default().push(i as u32);
            }
            queue.push_back(i);
            queued[i] = true;
        }
        Self {
            known,
            totals,
            first_pass: queue.len(),
            queue,
            is_waiting: vec![false; queued.len()],
            queued,
            waiting: Vec::new(),
            by_cell,
            stepped: 0,
            sweep: 0,
        }
    }

    pub fn summary(&self) -> Option<&Summary> {
        (self.first_pass == 0).then_some(&self.totals)
    }

    pub fn open(&self) -> u32 {
        self.totals.open
    }

    fn enqueue(&mut self, unit: usize) {
        enqueue(&mut self.queue, &mut self.queued, unit);
    }

    pub fn step(
        &mut self,
        design: &Design,
        budget: usize,
        now: u64,
        stagger: u64,
        changed: &[[i32; 3]],
        lost: bool,
    ) {
        let behind = lost || self.stepped + 1 != now;
        self.stepped = now;
        if behind {
            for unit in 0..self.known.len() {
                if !matches!(design.plan(design.units[unit]), Plan::Unsupported(_)) {
                    self.enqueue(unit);
                }
            }
        }
        for cell in changed {
            for unit in self.by_cell.get(cell).into_iter().flatten() {
                enqueue(&mut self.queue, &mut self.queued, *unit as usize);
            }
        }
        if RETRY.due(now, stagger) {
            for unit in std::mem::take(&mut self.waiting) {
                self.is_waiting[unit] = false;
                self.enqueue(unit);
            }
        }
        let sweep = if SWEEP_CADENCE.due(now, stagger) {
            SWEEP
        } else {
            0
        };
        for _ in 0..sweep.min(self.known.len()) {
            self.sweep = (self.sweep + 1) % self.known.len();
            if !matches!(design.plan(design.units[self.sweep]), Plan::Unsupported(_)) {
                self.enqueue(self.sweep);
            }
        }
        let n = budget.min(self.queue.len());
        if n == 0 {
            return;
        }
        let batch: Vec<usize> = self.queue.drain(..n).collect();
        for &unit in &batch {
            self.queued[unit] = false;
        }
        self.measure(design, &batch);
        self.first_pass = self.first_pass.saturating_sub(n);
    }

    pub fn measure(&mut self, design: &Design, indices: &[usize]) {
        let asks: Vec<(usize, ([i32; 3], BlockRecord))> = indices
            .iter()
            .filter(|&&i| !matches!(design.plan(design.units[i]), Plan::Unsupported(_)))
            .map(|&i| {
                let unit = design.units[i];
                (i, (unit.pos, design.records[unit.record as usize].clone()))
            })
            .collect();
        let (which, cells): (Vec<usize>, Vec<_>) = asks.into_iter().unzip();
        let statuses = paged(cells, block_record_statuses);
        for (i, status) in which.into_iter().zip(statuses) {
            let known = Known::of(status);
            if matches!(known, Known::Unloaded | Known::Pending)
                && !std::mem::replace(&mut self.is_waiting[i], true)
            {
                self.waiting.push(i);
            }
            if known != self.known[i] {
                let cost = design.cost(design.units[i]);
                self.totals.count(&self.known[i], cost, false);
                self.totals.count(&known, cost, true);
                self.known[i] = known;
            }
        }
    }
}

fn enqueue(queue: &mut VecDeque<usize>, queued: &mut [bool], unit: usize) {
    if !std::mem::replace(&mut queued[unit], true) {
        queue.push_back(unit);
    }
}

#[cfg(test)]
mod tests;
