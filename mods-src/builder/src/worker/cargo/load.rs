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

pub(super) fn slots_used(
    ctx: &mut Ctx,
    slots: &[Option<ItemStackData>],
    items: &BTreeMap<ItemKey, u32>,
) -> usize {
    let mut left = items.clone();
    let mut room: BTreeMap<ItemKey, u32> = BTreeMap::new();
    let mut used = 0;
    for stack in slots.iter().flatten() {
        if protected(ctx, stack) {
            used += 1;
            continue;
        }
        let key = key_of(stack);
        let keep = left.entry(key.clone()).or_default();
        let count = (*keep).min(u32::from(stack.count));
        if count > 0 {
            used += 1;
            *keep -= count;
            *room.entry(key).or_default() +=
                u32::from(ctx.caches.max_stack(&stack.item)).saturating_sub(count);
        }
    }
    for (key, count) in left {
        let extra = count.saturating_sub(room.get(&key).copied().unwrap_or(0));
        used += (extra as usize).div_ceil(ctx.caches.max_stack(&key.0) as usize);
    }
    used
}

pub(super) fn fill(
    ctx: &mut Ctx,
    slots: &[Option<ItemStackData>],
    items: &mut BTreeMap<ItemKey, u32>,
    key: ItemKey,
    count: u32,
    room: usize,
) {
    let held = items.get(&key).copied().unwrap_or(0);
    let mut low = held;
    let mut high = count.max(held);
    while low < high {
        let count = low + (high - low).div_ceil(2);
        let mut next = items.clone();
        next.insert(key.clone(), count);
        if slots_used(ctx, slots, &next) <= room {
            low = count;
        } else {
            high = count - 1;
        }
    }
    if low > held {
        items.insert(key, low);
    }
}
