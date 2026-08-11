//! THE ANVIL IS A WORKSTATION, NOT A CONTAINER (Rachel, 2026-08-08): nothing
//! rests in it. Pulling the tool out sends the staged materials after it, a
//! swapped-in tool ejects whatever it cannot host, and the last viewer to
//! close the panel takes everything home — inventory first, overflow dropped
//! at that player, spilled above the anvil only when nobody resolvable is
//! left (contents are never destroyed for nothing).
//!
//! The delivery POLICY lives here; `step` decides when each sweep runs.

use mod_sdk::*;

use machine_core::StepCtx;

use crate::anvil::{AnvilSpec, CellState, SLOTS};

/// Remove `n` items from a slot (empties it at zero).
pub(super) fn take(slot: &mut Option<ItemStackData>, n: u8) {
    if let Some(s) = slot {
        s.count = s.count.saturating_sub(n);
        if s.count == 0 {
            *slot = None;
        }
    }
}

/// Hand `stack` to `player` — inventory first, overflow dropped at the
/// player. `false` = no such connected session (the stack was not moved).
fn give_stack_to(player: PlayerId, stack: &ItemStackData) -> bool {
    let data: Vec<(&str, &[u8])> = stack
        .data
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_slice()))
        .collect();
    give_item_to(player, &stack.item, stack.count, &data)
}

/// Spill `stack` into the world above the anvil at `pos` — the fallback when
/// nobody is left to hand it to. Contents are never destroyed for nothing.
fn spill_stack(pos: [i32; 3], stack: &ItemStackData) {
    let at = [
        pos[0] as f32 + 0.5,
        pos[1] as f32 + 1.2,
        pos[2] as f32 + 0.5,
    ];
    let data: Vec<(&str, &[u8])> = stack
        .data
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_slice()))
        .collect();
    spawn_item_data(&stack.item, stack.count, at, &data);
}

impl AnvilSpec {
    /// Send the contents of `cells` home to a CURRENT viewer — the player
    /// who just pulled (or swapped) the tool takes the staged materials
    /// with it. A stack that cannot be delivered stays put (someone is
    /// watching, so this only means a race with a disconnect; the close
    /// path catches it).
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

    /// Empty `cells` outright: to `player` when one is known and still
    /// connected, spilling at the anvil otherwise. The close-of-session
    /// sweep — after it the machine holds nothing.
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

    /// The socket cells whose resting stacks the tool in the slot cannot
    /// host: every occupied cell whose state is not OPEN for THIS tool —
    /// and every occupied cell at all when the tool is unreadable (unknown
    /// row or a record this build refuses). Runs after carving, so a gem
    /// mid-carve-gesture is never here: leftover gems can only rest in an
    /// open cell (inert, like a misfit) or an absent one (ejected).
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
