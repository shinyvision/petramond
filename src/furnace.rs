//! The furnace block-entity: per-furnace smelting state, owned by the chunk it
//! sits in (see [`Chunk::furnaces`](crate::chunk::Chunk)).
//!
//! A furnace holds three item slots — `input` (the smeltable, top), `fuel`
//! (bottom), and `output` (right) — plus the in-progress cook and burn timers.
//! It advances one step per fixed game tick (20 TPS) via [`Furnace::tick`].
//! "Lit" — which drives the burning texture — is simply `burn_remaining > 0`, a
//! *derived* property rather than stored state, so it can never disagree with the
//! actual burn timer.
//!
//! Fuel burn time is a property of the fuel item ([`ItemType::fuel_burn_ticks`]);
//! what smelts into what is supplied by the caller as a closure, since the recipe
//! set lives in `crafting` and the storage layer must not depend on it.

use crate::inventory::Inventory;
use crate::item::{ItemStack, ItemTag, ItemType};
use crate::render::FurnaceView;

/// Game ticks to smelt one item (30 s at 20 TPS), matching Minecraft.
pub const COOK_TICKS: u16 = 600;

/// How far the cook bar slides back per tick when smelting stalls (input/output
/// missing). Minecraft eases the arrow back rather than snapping it to empty.
const COOK_REGRESS: u16 = 2;

/// Which horizontal direction a furnace's front (mouth) faces. Set when the block
/// is placed so the mouth points toward the player, and read by the mesher to put
/// the front texture on that one face and `furnace_side` on the other three.
#[repr(u8)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Facing {
    #[default]
    North = 0, // front faces -Z
    South = 1, // +Z
    West = 2,  // -X
    East = 3,  // +X
}

impl Facing {
    /// Numeric tag, for saving.
    #[inline]
    pub fn to_u8(self) -> u8 {
        self as u8
    }

    /// Restore from a saved tag (unknown values fall back to `North`).
    #[inline]
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Facing::South,
            2 => Facing::West,
            3 => Facing::East,
            _ => Facing::North,
        }
    }
}

/// Which fillable slot a shift-clicked stack belongs in. The two *fillable* roles —
/// the only ones a player can deposit into. The output is deliberately absent: it is
/// a take-only slot (you remove the finished product, you can never put into it), so
/// it has no place in a routing decision that picks a deposit target.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FillSlot {
    /// The top slot — what gets smelted (a [`Smeltable`](ItemTag::Smeltable) item).
    Input,
    /// The bottom slot — what burns (a [`Fuel`](ItemTag::Fuel) item).
    Fuel,
}

/// One furnace's contents and smelting progress. POD: small and `Copy`, so the
/// chunk can store it by value and the tick can snapshot it to detect change.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Furnace {
    /// The item being smelted (top slot).
    pub input: Option<ItemStack>,
    /// The fuel (bottom slot).
    pub fuel: Option<ItemStack>,
    /// The finished product (right slot) — take-only in the UI.
    pub output: Option<ItemStack>,
    /// Ticks of progress on the current item (`0..COOK_TICKS`).
    pub cook_progress: u16,
    /// Ticks of fuel left to burn. `> 0` means lit.
    pub burn_remaining: u16,
    /// Total burn time of the fuel currently being consumed, for the flame gauge.
    pub burn_max: u16,
    /// Which way the front faces (placement orientation). Rendering only.
    pub facing: Facing,
}

impl Furnace {
    /// Whether the furnace is currently burning fuel (drives the lit texture).
    #[inline]
    pub fn is_lit(&self) -> bool {
        self.burn_remaining > 0
    }

    /// `true` when every slot is empty (used when breaking the block — nothing to
    /// drop — and to prune furnaces that no longer need saving).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.input.is_none() && self.fuel.is_none() && self.output.is_none()
    }

    /// Cook progress as a `0.0..=1.0` fraction (drives the GUI arrow).
    #[inline]
    pub fn cook_fraction(&self) -> f32 {
        self.cook_progress as f32 / COOK_TICKS as f32
    }

    /// Remaining fuel as a `0.0..=1.0` fraction of the current fuel's full burn
    /// (drives the GUI flame); `0.0` when not lit.
    #[inline]
    pub fn burn_fraction(&self) -> f32 {
        if self.burn_max == 0 {
            0.0
        } else {
            self.burn_remaining as f32 / self.burn_max as f32
        }
    }

    /// A snapshot of this furnace for the open-furnace screen: its three slots and
    /// the two progress gauges (`0.0..=1.0`). The renderer takes it by value (all
    /// `Copy`), so it holds no borrow on the furnace.
    pub fn view(&self) -> FurnaceView {
        FurnaceView {
            input: self.input,
            fuel: self.fuel,
            output: self.output,
            cook01: self.cook_fraction(),
            burn01: self.burn_fraction(),
        }
    }

    /// Click the **take-only** output: move the whole finished product onto `cursor`
    /// if it fits (cursor empty, or the same item with room for the entire stack),
    /// clearing the output and returning `true`. Returns `false` (touching nothing)
    /// when the output is empty or won't fit.
    ///
    /// This is the *only* way to interact with the output: there is deliberately no
    /// method that deposits into it. The output slot accumulates smelted product
    /// (via [`produce`](Self::produce) inside [`tick`](Self::tick)) and the player
    /// only ever takes from it — the take-only rule the UI relies on.
    pub fn take_output(&mut self, cursor: &mut Option<ItemStack>) -> bool {
        let Some(out) = self.output else {
            return false;
        };
        if stack_onto_cursor(cursor, out) {
            self.output = None;
            true
        } else {
            false
        }
    }

    /// Where a shift-clicked stack of `item` belongs, by its item *tags*: a
    /// [`Fuel`](ItemTag::Fuel) item heads for the fuel slot and a
    /// [`Smeltable`](ItemTag::Smeltable) item for the input slot. `None` means the
    /// furnace wants neither (the caller does its ordinary inventory shuffle).
    ///
    /// This tag routing is *furnace* behavior — it reads `ItemTag` from item data —
    /// and lives here rather than in any shared container. The output is never a
    /// destination: it is take-only.
    pub fn fill_slot_for(item: ItemType) -> Option<FillSlot> {
        if item.has_tag(ItemTag::Fuel) {
            Some(FillSlot::Fuel)
        } else if item.has_tag(ItemTag::Smeltable) {
            Some(FillSlot::Input)
        } else {
            None
        }
    }

    /// Shift a stack into one of the fillable slots: merge `src` onto a matching
    /// stack up to its max, or fill the slot if empty; whatever doesn't fit stays in
    /// `src`. A different item already in the slot blocks the move. No-op on an empty
    /// `src`. The `role` is chosen by [`fill_slot_for`](Self::fill_slot_for).
    pub fn shift_in(&mut self, role: FillSlot, src: &mut Option<ItemStack>) {
        let dst = match role {
            FillSlot::Input => &mut self.input,
            FillSlot::Fuel => &mut self.fuel,
        };
        merge_stack(src, dst);
    }

    /// Mutable handle to the input (smeltable, top) slot, for the caller's cursor
    /// click rule (pick / drop / merge / swap) and shift-out. A fillable slot.
    #[inline]
    pub fn input_slot(&mut self) -> &mut Option<ItemStack> {
        &mut self.input
    }

    /// Mutable handle to the fuel (bottom) slot, for the caller's cursor click rule
    /// and shift-out. A fillable slot.
    #[inline]
    pub fn fuel_slot(&mut self) -> &mut Option<ItemStack> {
        &mut self.fuel
    }

    /// Shift-click the **take-only** output: move the finished product into `inv`
    /// (first-fit; whatever doesn't fit stays in the output). Take-only out — like
    /// [`take_output`](Self::take_output) it only ever removes product, never accepts
    /// a deposit. The output has no general mutable handle, so there is *no* way to
    /// put a stack into it: the only producer is the smelt step inside
    /// [`tick`](Self::tick).
    pub fn shift_output_into(&mut self, inv: &mut Inventory) {
        inv.pull_from(&mut self.output);
    }

    /// The result of smelting the current input once, or `None` if the input is
    /// empty or not smeltable. `smelt` resolves an item to its smelted product.
    fn smelt_result(&self, smelt: &impl Fn(ItemType) -> Option<ItemStack>) -> Option<ItemStack> {
        let input = self.input?;
        if input.is_empty() {
            return None;
        }
        smelt(input.item)
    }

    /// Whether `result` can be merged into the output slot (empty, or the same item
    /// with room for the whole result).
    fn output_accepts(&self, result: ItemStack) -> bool {
        match self.output {
            None => true,
            Some(o) => o.item == result.item && o.space_left() >= result.count,
        }
    }

    /// Whether the furnace has something to smelt AND somewhere to put it.
    fn can_smelt(&self, smelt: &impl Fn(ItemType) -> Option<ItemStack>) -> bool {
        match self.smelt_result(smelt) {
            Some(r) => self.output_accepts(r),
            None => false,
        }
    }

    /// Finish one smelt: deposit `result` and consume one input.
    fn produce(&mut self, result: ItemStack) {
        match &mut self.output {
            Some(o) => o.count += result.count,
            None => self.output = Some(result),
        }
        if let Some(input) = self.input.as_mut() {
            input.count -= 1;
            if input.count == 0 {
                self.input = None;
            }
        }
    }

    /// Consume one fuel item, clearing the slot when it empties.
    fn consume_fuel(&mut self) {
        if let Some(fuel) = self.fuel.as_mut() {
            fuel.count -= 1;
            if fuel.count == 0 {
                self.fuel = None;
            }
        }
    }

    /// Advance the furnace one game tick. Returns whether any state changed, so the
    /// caller can mark the owning chunk for save (any change) and re-mesh (a lit
    /// flip). `smelt(item)` yields the smelted product of an item, or `None`.
    ///
    /// Order: burn down the current fuel, (re)light from fresh fuel only when there
    /// is something to smelt, then advance — or regress — the cook bar.
    pub fn tick(&mut self, smelt: impl Fn(ItemType) -> Option<ItemStack>) -> bool {
        let before = *self;

        if self.burn_remaining > 0 {
            self.burn_remaining -= 1;
        }

        let can = self.can_smelt(&smelt);

        // Relight when the flame just went out and there is work to do — never burn
        // fuel on an idle furnace.
        if self.burn_remaining == 0 && can {
            if let Some(fuel) = self.fuel {
                let burn = fuel.item.fuel_burn_ticks();
                if burn > 0 {
                    self.burn_max = burn;
                    self.burn_remaining = burn;
                    self.consume_fuel();
                }
            }
        }

        if self.burn_remaining > 0 && can {
            self.cook_progress += 1;
            if self.cook_progress >= COOK_TICKS {
                if let Some(result) = self.smelt_result(&smelt) {
                    self.produce(result);
                }
                self.cook_progress = 0;
            }
        } else {
            self.cook_progress = self.cook_progress.saturating_sub(COOK_REGRESS);
        }

        *self != before
    }
}

/// Place a whole `stack` onto a cursor-held stack: succeeds (returns `true`) when the
/// cursor is empty (it becomes `stack`) or holds the same item with room for the
/// ENTIRE stack; otherwise leaves the cursor untouched and returns `false`. The
/// take-only-output rule: a product moves onto the cursor only if it fits in full.
/// Mirrors [`Inventory::try_stack_onto_cursor`](crate::inventory::Inventory) for a
/// bare cursor cell.
fn stack_onto_cursor(cursor: &mut Option<ItemStack>, stack: ItemStack) -> bool {
    match cursor {
        None => {
            *cursor = Some(stack);
            true
        }
        Some(cur) if cur.can_stack_with(&stack) && cur.space_left() >= stack.count => {
            cur.count += stack.count;
            true
        }
        _ => false,
    }
}

/// Move `src`'s stack into `dst` (one furnace slot): merge onto a matching item up to
/// its max, or fill an empty slot; whatever doesn't fit stays in `src`. A different
/// item in `dst` blocks the move. No-op on an empty `src`.
fn merge_stack(src: &mut Option<ItemStack>, dst: &mut Option<ItemStack>) {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Test smelter: raw iron -> 1 iron ingot, raw copper -> 1 copper ingot.
    fn smelt(item: ItemType) -> Option<ItemStack> {
        match item {
            ItemType::RawIron => Some(ItemStack::new(ItemType::IronIngot, 1)),
            ItemType::RawCopper => Some(ItemStack::new(ItemType::CopperIngot, 1)),
            _ => None,
        }
    }

    fn run(f: &mut Furnace, n: u32) {
        for _ in 0..n {
            f.tick(smelt);
        }
    }

    fn stack(item: ItemType, n: u8) -> Option<ItemStack> {
        Some(ItemStack::new(item, n))
    }

    #[test]
    fn empty_furnace_does_nothing() {
        let mut f = Furnace::default();
        assert!(!f.tick(smelt), "an idle furnace reports no change");
        assert!(!f.is_lit());
        assert_eq!(f, Furnace::default());
    }

    #[test]
    fn smelts_one_item_in_600_ticks() {
        let mut f = Furnace {
            input: stack(ItemType::RawIron, 1),
            fuel: stack(ItemType::Coal, 1),
            ..Default::default()
        };
        // Lights on the first tick and begins cooking.
        assert!(f.tick(smelt));
        assert!(f.is_lit(), "furnace lights from the coal");
        assert_eq!(f.burn_max, 4800);
        // 599 more ticks complete the first (and only) smelt at tick 600.
        run(&mut f, 599);
        assert_eq!(f.output, stack(ItemType::IronIngot, 1));
        assert!(f.input.is_none(), "the single raw iron is consumed");
        assert_eq!(f.cook_progress, 0);
    }

    #[test]
    fn one_coal_smelts_eight_items() {
        let mut f = Furnace {
            input: stack(ItemType::RawIron, 64),
            fuel: stack(ItemType::Coal, 1),
            ..Default::default()
        };
        // One coal burns 4800 ticks = eight 600-tick smelts.
        run(&mut f, 4800);
        assert_eq!(f.output.unwrap().count, 8, "4800 / 600 = 8 ingots");
        assert_eq!(f.input.unwrap().count, 56, "64 - 8 consumed");
        // Fuel is spent; one more tick puts the flame out.
        f.tick(smelt);
        assert!(!f.is_lit(), "the flame goes out once the coal is spent");
        assert!(f.fuel.is_none());
    }

    #[test]
    fn flame_relights_from_a_second_coal() {
        let mut f = Furnace {
            input: stack(ItemType::RawIron, 64),
            fuel: stack(ItemType::Coal, 2),
            ..Default::default()
        };
        // Past the first coal's burn: it relights from the second, staying lit.
        run(&mut f, 4810);
        assert!(f.is_lit(), "second coal keeps it burning");
        assert_eq!(f.fuel, None, "both coal eventually consumed");
    }

    #[test]
    fn cook_regresses_when_input_runs_out_mid_smelt() {
        let mut f = Furnace {
            input: stack(ItemType::RawIron, 1),
            fuel: stack(ItemType::Coal, 1),
            ..Default::default()
        };
        run(&mut f, 1); // light + start cooking
        run(&mut f, 100); // cook to ~101
        assert!(f.cook_progress > 0);
        // The single item already finished? No — 101 < 600, still cooking. Pull the
        // input out: the cook bar now slides back even though the fuel keeps burning.
        f.input = None;
        let progress = f.cook_progress;
        f.tick(smelt);
        assert!(f.cook_progress < progress, "cook bar eases back when nothing smelts");
        assert!(f.is_lit(), "already-lit fuel keeps burning");
    }

    #[test]
    fn full_output_stops_smelting() {
        let mut f = Furnace {
            input: stack(ItemType::RawIron, 5),
            fuel: stack(ItemType::Coal, 1),
            output: stack(ItemType::IronIngot, 64), // no room
            ..Default::default()
        };
        run(&mut f, 600);
        assert_eq!(f.output.unwrap().count, 64, "no ingot added when output is full");
        assert_eq!(f.input.unwrap().count, 5, "no input consumed");
        assert!(!f.is_lit(), "never lit: there was nothing it could smelt");
    }

    #[test]
    fn non_fuel_in_fuel_slot_does_not_burn() {
        let mut f = Furnace {
            input: stack(ItemType::RawIron, 1),
            fuel: stack(ItemType::Dirt, 1), // not a fuel
            ..Default::default()
        };
        run(&mut f, 600);
        assert!(!f.is_lit());
        assert!(f.output.is_none(), "nothing smelts without real fuel");
        assert_eq!(f.fuel, stack(ItemType::Dirt, 1), "non-fuel is left untouched");
    }
}
