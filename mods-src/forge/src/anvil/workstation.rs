use mod_sdk::*;

use machine_core::StepCtx;

use crate::anvil::{AnvilSpec, CellState, SLOTS};

pub(super) fn take(slot: &mut Option<ItemStackData>, n: u8) {
    if let Some(s) = slot {
        s.count = s.count.saturating_sub(n);
        if s.count == 0 {
            *slot = None;
        }
    }
}

fn give_stack_to(player: PlayerId, stack: &ItemStackData) -> bool {
    let data: Vec<(&str, &[u8])> = stack
        .data
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_slice()))
        .collect();
    give_item_to(player, &stack.item, stack.count, &data)
}

fn spill_stack(pos: [i32; 3], stack: &ItemStackData) {
    let at = [
        pos[0] as f64 + 0.5,
        pos[1] as f64 + 1.2,
        pos[2] as f64 + 0.5,
    ];
    let data: Vec<(&str, &[u8])> = stack
        .data
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_slice()))
        .collect();
    spawn_item_data(&stack.item, stack.count, at, &data);
}

impl AnvilSpec {
    pub(super) fn return_cells(
        &self,
        ctx: &StepCtx<'_>,
        slots: &mut [Option<ItemStackData>],
        cells: impl IntoIterator<Item = usize>,
    ) {
        let Some(player) = ctx.viewers.first().copied() else {
            return;
        };
        for cell in cells {
            let Some(stack) = slots[cell].take() else {
                continue;
            };
            if !give_stack_to(player, &stack) {
                slots[cell] = Some(stack);
            }
        }
    }

    pub(super) fn deliver_cells(
        &self,
        ctx: &StepCtx<'_>,
        slots: &mut [Option<ItemStackData>],
        cells: std::ops::Range<usize>,
        player: Option<PlayerId>,
    ) {
        for cell in cells {
            let Some(stack) = slots[cell].take() else {
                continue;
            };
            if !player.is_some_and(|p| give_stack_to(p, &stack)) {
                spill_stack(ctx.pos, &stack);
            }
        }
    }

    pub(super) fn cells_to_eject(&self, slots: &[Option<ItemStackData>]) -> Vec<usize> {
        let occupied = (1..SLOTS).filter(|&c| slots[c].is_some());
        match self.tool_in(slots) {
            Some((_, tool_slots, _, rec)) => occupied
                .filter(|&cell| {
                    !matches!(
                        Self::cell_state(tool_slots, &rec, cell - 1),
                        CellState::Open
                    )
                })
                .collect(),
            None => occupied.collect(),
        }
    }
}
