use std::collections::{BTreeMap, BTreeSet};

use crate::content::BLUEPRINT;
use crate::geometry::manhattan;
use crate::host::prelude::*;
use crate::survey::{key_of, ItemKey};
use crate::worker::{Body, Ctx};

use super::totals;
use super::wants::tool_kind;

pub(super) struct Load {
    pub items: BTreeMap<ItemKey, u32>,
    pub tools: BTreeSet<String>,
}

impl Load {
    pub fn container(
        &self,
        ctx: &mut Ctx,
        body: &Body,
        stock: &crate::supplies::Stock,
    ) -> Option<[i32; 3]> {
        let need = self.missing(&body.slots);
        stock
            .containers
            .iter()
            .zip(&stock.slots)
            .filter(|(_, slots)| {
                slots.iter().flatten().any(|s| {
                    need.contains_key(&key_of(s))
                        || tool_kind(ctx, &s.item)
                            .is_some_and(|(kind, _)| self.tools.contains(&kind))
                })
            })
            .map(|(cell, _)| *cell)
            .min_by_key(|cell| manhattan(*cell, body.cell))
    }

    pub fn missing(&self, slots: &[Option<ItemStackData>]) -> BTreeMap<ItemKey, u32> {
        let carried = totals(slots);
        self.items
            .iter()
            .filter_map(|(key, n)| {
                let missing = n.saturating_sub(carried.get(key).copied().unwrap_or(0));
                (missing > 0).then(|| (key.clone(), missing))
            })
            .collect()
    }

    pub fn returns(&self, ctx: &mut Ctx, slots: &[Option<ItemStackData>]) -> Vec<(usize, u8)> {
        let mut left = self.items.clone();
        slots
            .iter()
            .enumerate()
            .filter_map(|(slot, stack)| {
                let stack = stack.as_ref()?;
                if protected(ctx, stack) {
                    return None;
                }
                let keep = left.entry(key_of(stack)).or_default();
                let count = u32::from(stack.count);
                let back = count.saturating_sub(*keep);
                *keep = keep.saturating_sub(count);
                (back > 0).then_some((slot, back as u8))
            })
            .collect()
    }
}

fn protected(ctx: &mut Ctx, stack: &ItemStackData) -> bool {
    stack.item == BLUEPRINT || tool_kind(ctx, &stack.item).is_some()
}

/// The slots a load takes. An item's share depends only on its own count and the stacks of it
/// already carried, so a load can be priced item by item without recounting the rest.
pub(super) struct Packing {
    protected: usize,
    carried: BTreeMap<ItemKey, (Vec<u32>, u32)>,
}

impl Packing {
    pub fn new(ctx: &mut Ctx, slots: &[Option<ItemStackData>]) -> Self {
        let mut protected_slots = 0;
        let mut carried: BTreeMap<ItemKey, (Vec<u32>, u32)> = BTreeMap::new();
        for stack in slots.iter().flatten() {
            if protected(ctx, stack) {
                protected_slots += 1;
                continue;
            }
            let max = u32::from(ctx.caches.max_stack(&stack.item));
            let entry = carried.entry(key_of(stack)).or_insert((Vec::new(), max));
            entry.0.push(u32::from(stack.count));
        }
        Self {
            protected: protected_slots,
            carried,
        }
    }

    /// Slots `count` of `key` takes, topping up carried stacks before opening new ones.
    pub fn slots(&self, ctx: &mut Ctx, key: &ItemKey, count: u32) -> usize {
        let mut left = count;
        let mut used = 0;
        let mut room = 0u32;
        if let Some((stacks, max)) = self.carried.get(key) {
            for &held in stacks {
                let take = left.min(held);
                if take > 0 {
                    used += 1;
                    left -= take;
                    room += max.saturating_sub(take);
                }
            }
        }
        let extra = left.saturating_sub(room);
        used + (extra as usize).div_ceil(ctx.caches.max_stack(&key.0) as usize)
    }

    pub fn total(&self, ctx: &mut Ctx, items: &BTreeMap<ItemKey, u32>) -> usize {
        self.protected
            + items
                .iter()
                .map(|(key, count)| self.slots(ctx, key, *count))
                .sum::<usize>()
    }
}

pub(super) fn fill(
    ctx: &mut Ctx,
    slots: &[Option<ItemStackData>],
    items: &mut BTreeMap<ItemKey, u32>,
    key: ItemKey,
    count: u32,
    room: usize,
) {
    let packing = Packing::new(ctx, slots);
    let held = items.get(&key).copied().unwrap_or(0);
    let others = packing.total(ctx, items) - packing.slots(ctx, &key, held);
    let mut low = held;
    let mut high = count.max(held);
    while low < high {
        let count = low + (high - low).div_ceil(2);
        if others + packing.slots(ctx, &key, count) <= room {
            low = count;
        } else {
            high = count - 1;
        }
    }
    if low > held {
        items.insert(key, low);
    }
}
