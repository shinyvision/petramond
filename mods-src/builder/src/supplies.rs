//! The storage a table draws from: supply blocks touching the table, and
//! supply blocks touching those, as one chain.
//!
//! Which blocks count is row data (`builder:supply`), so any pack's storage
//! joins by patching its row. The table's own blueprint slot never counts.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::rc::Rc;

use mod_sdk::*;

use crate::fx::{HashMap, HashSet};
use crate::geometry::FACES;
use crate::survey::{key_of, ItemKey};

/// The most containers one chain reaches.
pub const MAX_CONTAINERS: usize = 32;
const SUPPLY_DATA: &str = "builder:supply";

/// A table's stock and the tick it was read.
type ReadStock = (u64, Rc<Stock>);

pub struct Supplies {
    kinds: HashSet<BlockId>,
    /// Chains already walked, until a cell they looked at changes.
    chains: RefCell<HashMap<[i32; 3], Chain>>,
    /// What each table's chain held, and the tick it was read.
    stocks: RefCell<HashMap<[i32; 3], ReadStock>>,
}

struct Chain {
    containers: Vec<[i32; 3]>,
    /// Every cell the walk asked about: a change to any of them is a change
    /// to the chain.
    probed: HashSet<[i32; 3]>,
    /// Asked for since the last sweep.
    used: bool,
}

/// One chain's contents, read once.
#[derive(Default)]
pub struct Stock {
    pub containers: Vec<[i32; 3]>,
    pub slots: Vec<Vec<Option<ItemStackData>>>,
    pub totals: BTreeMap<ItemKey, u32>,
    /// Whether every container answered. One out of the loaded world reads as
    /// empty, and empty chests are not the same as chests nobody can see.
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

    /// Every supply container chained to `table`, nearest first. Cells that
    /// are not stream-final end the chain there, like air.
    pub fn chain(&self, table: [i32; 3]) -> Vec<[i32; 3]> {
        if let Some(chain) = self.chains.borrow_mut().get_mut(&table) {
            chain.used = true;
            return chain.containers.clone();
        }
        let (chain, settled) = self.walk(table);
        let containers = chain.containers.clone();
        // A walk that met ground still streaming in is no answer to keep:
        // streaming is no change, so nothing would ever correct it.
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

    /// Take in the cells the world's change log names (`lost`: some are
    /// unknown): chains that looked at one of them are walked again.
    pub fn changed(&self, cells: &[[i32; 3]], lost: bool) {
        let mut chains = self.chains.borrow_mut();
        if lost {
            chains.clear();
        } else if !cells.is_empty() {
            chains.retain(|_, chain| !cells.iter().any(|cell| chain.probed.contains(cell)));
        }
    }

    /// Let go of chains and stock nobody asked for since the last sweep.
    pub fn sweep(&self, now: u64) {
        self.chains
            .borrow_mut()
            .retain(|_, chain| std::mem::take(&mut chain.used));
        self.stocks.borrow_mut().retain(|_, (at, _)| *at == now);
    }

    /// The chain's contents as they are right now.
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

    /// The chain's contents, read at most once a tick per table: for the
    /// panels and admission, which ask about the same table many times over
    /// and move nothing themselves.
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

/// What a bill wants beyond what there is: the worst of it, in words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shortfall {
    pub count: u32,
    pub name: String,
    /// Other items are short too.
    pub more: bool,
}

impl fmt::Display for Shortfall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let more = if self.more { " and more" } else { "" };
        write!(
            f,
            "{}{}x {}{more}",
            crate::project::note::MISSING,
            self.count,
            self.name
        )
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

/// The largest gap of a [`shortfall`], named for the owner.
pub fn worst(short: &[(ItemKey, u32)], caches: &mut crate::caches::Caches) -> Option<Shortfall> {
    let ((item, _), count) = short.first()?;
    Some(Shortfall {
        count: *count,
        name: caches.display_name(item),
        more: short.len() > 1,
    })
}
