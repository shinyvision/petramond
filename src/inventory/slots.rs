use crate::item::{ItemStack, ItemType};
pub(crate) fn insert_into_slots(
    slots: &mut [Option<ItemStack>],
    mut stack: ItemStack,
) -> Option<ItemStack> {
    if stack.is_empty() {
        return None;
    }

    // Pass 1: top up existing matching, non-full stacks in slot order.
    for existing in slots.iter_mut().flatten() {
        if existing.can_stack_with(&stack) {
            let space = existing.space_left();
            if space > 0 {
                let moved = space.min(stack.count);
                existing.count += moved;
                stack.count -= moved;
                if stack.count == 0 {
                    return None;
                }
            }
        }
    }

    // Pass 2: drop the remainder into empty slots, one full stack at a time.
    for slot in slots.iter_mut() {
        if slot.is_none() {
            let put = stack.count.min(stack.item.max_stack_size());
            *slot = Some(ItemStack::new(stack.item, put));
            stack.count -= put;
            if stack.count == 0 {
                return None;
            }
        }
    }

    Some(stack)
}

/// How many items of `item` a storage cell can accept without swapping its
/// contents. This is the placement capacity used by slot-drag distribution.
pub(crate) fn slot_capacity(slot: &Option<ItemStack>, item: ItemType) -> u8 {
    match slot {
        None => item.max_stack_size(),
        Some(existing) if existing.item == item => existing.space_left(),
        Some(_) => 0,
    }
}

/// Plan the per-destination counts for one ordered slot-drag gesture. The
/// capacity callback filters incompatible/full cells before primary-button
/// division, while actual placement remains responsible for capacity limits.
/// This pure plan is shared by authoritative mutation and its visual preview.
pub(crate) fn plan_drag_distribution<T: Copy + Eq>(
    hits: &[T],
    held_count: u8,
    one_each: bool,
    mut capacity: impl FnMut(T) -> u8,
) -> Vec<(T, u8)> {
    if held_count == 0 {
        return Vec::new();
    }

    let mut destinations = Vec::new();
    for &slot in hits {
        if !destinations.contains(&slot) && capacity(slot) > 0 {
            destinations.push(slot);
        }
    }
    if destinations.is_empty() {
        return Vec::new();
    }

    let (share, remainder) = if one_each {
        (1, 0)
    } else {
        let slots = destinations.len() as u32;
        (
            (u32::from(held_count) / slots) as u8,
            (u32::from(held_count) % slots) as u8,
        )
    };
    let last = destinations.len() - 1;
    destinations
        .into_iter()
        .enumerate()
        .filter_map(|(index, slot)| {
            let wanted = share + u8::from(index == last) * remainder;
            (wanted > 0).then_some((slot, wanted))
        })
        .collect()
}

/// Move at most `wanted` items from the cursor into a compatible storage
/// cell. Unlike an ordinary primary click this never swaps mismatched stacks.
pub(crate) fn place_cursor_count(
    cursor: &mut Option<ItemStack>,
    slot: &mut Option<ItemStack>,
    wanted: u8,
) -> u8 {
    let Some(mut held) = cursor.take() else {
        return 0;
    };
    let moved = wanted.min(held.count).min(slot_capacity(slot, held.item));
    if moved > 0 {
        match slot {
            None => *slot = Some(ItemStack::new(held.item, moved)),
            Some(existing) => existing.count += moved,
        }
        held.count -= moved;
    }
    *cursor = (held.count > 0).then_some(held);
    moved
}

/// Remove one item or a whole stack from a concrete slot.
pub(crate) fn take_slot_stack(slot: &mut Option<ItemStack>, all: bool) -> Option<ItemStack> {
    if all {
        return slot.take();
    }
    let stack = slot.as_mut()?;
    let item = stack.item;
    stack.count -= 1;
    if stack.count == 0 {
        *slot = None;
    }
    Some(ItemStack::new(item, 1))
}

/// Merge `src` into `dst`: fill an empty `dst`, top up a matching stack to
/// its cap (leaving the remainder in `src`), and leave both untouched on a
/// mismatch — the unit move behind every shift-route into container slots.
pub(crate) fn merge_stack(src: &mut Option<ItemStack>, dst: &mut Option<ItemStack>) {
    let Some(mut incoming) = src.take() else {
        return;
    };
    match dst {
        None => *dst = Some(incoming),
        Some(existing) if existing.can_stack_with(&incoming) => {
            let moved = existing.space_left().min(incoming.count);
            existing.count += moved;
            incoming.count -= moved;
            *src = (incoming.count > 0).then_some(incoming);
        }
        Some(_) => *src = Some(incoming),
    }
}
