use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::rc::Rc;

use crate::host::prelude::*;
use serde::{Deserialize, Serialize};

use crate::fx::{HashMap, HashSet};
use crate::geometry::FACES;
use crate::keys::SUPPLY_DATA;
use crate::survey::{key_of, ItemKey};

pub const MAX_CONTAINERS: usize = 32;

type ReadStock = (u64, Rc<Stock>);

pub struct Supplies {
    kinds: HashSet<BlockId>,
    chains: RefCell<HashMap<[i32; 3], Chain>>,
    stocks: RefCell<HashMap<[i32; 3], ReadStock>>,
}

struct Chain {
    containers: Vec<[i32; 3]>,
    probed: HashSet<[i32; 3]>,
    used: bool,
}

#[derive(Default)]
pub struct Stock {
    pub containers: Vec<[i32; 3]>,
    pub slots: Vec<Vec<Option<ItemStackData>>>,
    pub totals: BTreeMap<ItemKey, u32>,
    pub read: bool,
}

impl Stock {
    pub fn has_room(&self) -> bool {
        self.slots.iter().flatten().any(Option::is_none)
    }
}

impl Supplies {
    pub fn resolve() -> Self {
        Self {
            kinds: blocks_with_data(SUPPLY_DATA)
                .into_iter()
                .map(|(block, _)| block)
                .collect(),
            chains: RefCell::default(),
            stocks: RefCell::default(),
        }
    }

    pub fn chain(&self, table: [i32; 3]) -> Vec<[i32; 3]> {
        if let Some(chain) = self.chains.borrow_mut().get_mut(&table) {
            chain.used = true;
            return chain.containers.clone();
        }
        let (chain, settled) = self.walk(table);
        let containers = chain.containers.clone();
        if settled {
            self.chains.borrow_mut().insert(table, chain);
        }
        containers
    }

    fn walk(&self, table: [i32; 3]) -> (Chain, bool) {
        let mut found = Vec::new();
        let mut settled = true;
        let mut seen: HashSet<[i32; 3]> = [table].into_iter().collect();
        let mut anchors: HashSet<[i32; 3]> = HashSet::default();
        let mut frontier = VecDeque::from([table]);
        'walk: while let Some(cell) = frontier.pop_front() {
            let neighbours: Vec<[i32; 3]> = FACES
                .iter()
                .map(|f| [cell[0] + f[0], cell[1] + f[1], cell[2] + f[2]])
                .filter(|n| seen.insert(*n))
                .collect();
            for (n, block) in neighbours.iter().zip(get_blocks(neighbours.clone())) {
                settled &= block.is_some();
                if !block.is_some_and(|b| self.kinds.contains(&b)) {
                    continue;
                }
                let anchor = block_model_group(*n).map_or(*n, |g| g.base);
                if anchors.insert(anchor) {
                    found.push(anchor);
                    if found.len() == MAX_CONTAINERS {
                        break 'walk;
                    }
                }
                frontier.push_back(*n);
            }
        }
        let chain = Chain {
            containers: found,
            probed: seen,
            used: true,
        };
        (chain, settled)
    }

    pub fn changed(&self, cells: &[[i32; 3]], lost: bool) {
        let mut chains = self.chains.borrow_mut();
        if lost {
            chains.clear();
        } else if !cells.is_empty() {
            chains.retain(|_, chain| !cells.iter().any(|cell| chain.probed.contains(cell)));
        }
    }

    pub fn sweep(&self, now: u64) {
        self.chains
            .borrow_mut()
            .retain(|_, chain| std::mem::take(&mut chain.used));
        self.stocks.borrow_mut().retain(|_, (at, _)| *at == now);
    }

    pub fn stock(&self, table: [i32; 3]) -> Stock {
        let containers = self.chain(table);
        let answered = container_get_many(
            containers
                .iter()
                .map(|c| ContainerAddress::Block(*c))
                .collect(),
        );
        let read = answered.iter().all(Option::is_some);
        let slots: Vec<_> = answered
            .into_iter()
            .map(Option::unwrap_or_default)
            .collect();
        let mut totals = BTreeMap::new();
        for slots in &slots {
            add_totals(&mut totals, slots);
        }
        Stock {
            containers,
            slots,
            totals,
            read,
        }
    }

    pub fn stock_at(&self, table: [i32; 3], now: u64) -> Rc<Stock> {
        if let Some((at, stock)) = self.stocks.borrow().get(&table) {
            if *at == now {
                return Rc::clone(stock);
            }
        }
        let stock = Rc::new(self.stock(table));
        self.stocks
            .borrow_mut()
            .insert(table, (now, Rc::clone(&stock)));
        stock
    }
}

pub const MISSING: &str = "Missing ";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shortfall {
    pub count: u32,
    pub name: String,
    pub more: bool,
}

impl fmt::Display for Shortfall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let more = if self.more { " and more" } else { "" };
        write!(f, "{MISSING}{}x {}{more}", self.count, self.name)
    }
}

pub fn shortfall(
    bill: &BTreeMap<ItemKey, u32>,
    have: &BTreeMap<ItemKey, u32>,
) -> Vec<(ItemKey, u32)> {
    let mut short: Vec<(ItemKey, u32)> = bill
        .iter()
        .filter_map(|(key, need)| {
            let got = have.get(key).copied().unwrap_or(0);
            (got < *need).then(|| (key.clone(), need - got))
        })
        .collect();
    short.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    short
}

pub fn add_totals(into: &mut BTreeMap<ItemKey, u32>, slots: &[Option<ItemStackData>]) {
    for stack in slots.iter().flatten() {
        *into.entry(key_of(stack)).or_default() += u32::from(stack.count);
    }
}

pub fn worst(short: &[(ItemKey, u32)], caches: &mut crate::caches::Caches) -> Option<Shortfall> {
    let ((item, _), count) = short.first()?;
    Some(Shortfall {
        count: *count,
        name: caches.display_name(item),
        more: short.len() > 1,
    })
}

#[cfg(test)]
mod tests;
