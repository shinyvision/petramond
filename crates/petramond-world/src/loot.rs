//! Bounded weighted reward graphs. Callers own randomness and delivery.

use crate::item::{ItemStack, ItemType};
use crate::registry::Catalog;

mod load;
#[cfg(test)]
mod tests;

pub use load::{catalog, parse_layers};

const MAX_EXPANSION: usize = 256;
const MAX_DEPTH: usize = 16;

struct Pool {
    rolls: [u8; 2],
    weight: u32,
    entries: Vec<Entry>,
}

struct Entry {
    weight: u32,
    outcome: Outcome,
}

enum Outcome {
    Item(ItemType, [u8; 2]),
    Table(u16),
    Empty,
}

pub struct Table {
    pools: Vec<Pool>,
}

/// Immutable tables validated as an acyclic graph with bounded expansion.
pub struct Loot {
    tables: Catalog<Table>,
}

impl Loot {
    pub fn contains(&self, key: &str) -> bool {
        self.tables.id(key).is_some()
    }
    /// Roll without mutating inventory or world state. Equal random streams
    /// produce equal stacks, independent of who will eventually receive them.
    pub fn roll(&self, key: &str, mut random: impl FnMut() -> u64) -> Option<Vec<ItemStack>> {
        let id = self.tables.id(key)?;
        let mut output = Vec::new();
        self.expand(id, &mut random, &mut output);
        Some(output)
    }

    fn expand(&self, id: u16, random: &mut impl FnMut() -> u64, output: &mut Vec<ItemStack>) {
        for pool in &self.tables.rows()[id as usize].pools {
            for _ in 0..range(pool.rolls, random) {
                let mut ticket = (random() % pool.weight as u64) as u32;
                let selected = pool.entries.iter().find(|entry| {
                    if ticket < entry.weight {
                        true
                    } else {
                        ticket -= entry.weight;
                        false
                    }
                });
                match &selected.expect("validated pool weight").outcome {
                    Outcome::Item(item, count) => {
                        output.push(ItemStack::new(*item, range(*count, random)))
                    }
                    Outcome::Table(child) => self.expand(*child, random, output),
                    Outcome::Empty => {}
                }
            }
        }
    }
}

fn range([min, max]: [u8; 2], random: &mut impl FnMut() -> u64) -> u8 {
    min + (random() % (u64::from(max) - u64::from(min) + 1)) as u8
}
