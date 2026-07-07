//! The furnace's MACHINE STATE and cook algorithm. Its item slots are not
//! here: they live in the block's generic [`Container`](crate::container)
//! (slot convention [`SLOT_INPUT`]/[`SLOT_FUEL`]/[`SLOT_OUTPUT`]), the same
//! storage chests and mod containers use — the furnace only owns what makes
//! it a furnace: burn/cook counters and the rule for advancing them.

use crate::item::{ItemStack, ItemType};

pub const COOK_TICKS: u16 = 600;
const COOK_REGRESS: u16 = 2;

/// The furnace's slots within its container, in document order.
pub const SLOT_INPUT: usize = 0;
pub const SLOT_FUEL: usize = 1;
pub const SLOT_OUTPUT: usize = 2;
pub const FURNACE_SLOTS: usize = 3;

/// One furnace's burn/cook state. `Copy`; slots live in the block's container.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Furnace {
    pub cook_progress: u16,
    pub burn_remaining: u16,
    pub burn_max: u16,
}

impl Furnace {
    #[inline]
    pub fn is_lit(&self) -> bool {
        self.burn_remaining > 0
    }

    /// Advance one game tick over the furnace's container `slots`
    /// (input/fuel/output per the `SLOT_*` convention). Returns whether the
    /// state or any slot changed.
    pub fn tick(
        &mut self,
        slots: &mut [Option<ItemStack>],
        smelt: impl Fn(ItemType) -> Option<ItemStack>,
    ) -> bool {
        let before = *self;
        let slots_before: Vec<Option<ItemStack>> = slots.to_vec();

        if self.burn_remaining > 0 {
            self.burn_remaining -= 1;
        }

        let can = can_smelt(slots, &smelt);

        // Relight when the flame just went out and there is work to do — never
        // burn fuel on an idle furnace.
        if self.burn_remaining == 0 && can {
            if let Some(fuel) = slot(slots, SLOT_FUEL) {
                let burn = fuel.item.fuel_burn_ticks();
                if burn > 0 {
                    self.burn_max = burn;
                    self.burn_remaining = burn;
                    consume_one(slots, SLOT_FUEL);
                }
            }
        }

        if self.burn_remaining > 0 && can {
            self.cook_progress += 1;
            if self.cook_progress >= COOK_TICKS {
                if let Some(result) = smelt_result(slots, &smelt) {
                    produce(slots, result);
                }
                self.cook_progress = 0;
            }
        } else {
            self.cook_progress = self.cook_progress.saturating_sub(COOK_REGRESS);
        }

        *self != before || slots != slots_before
    }
}

#[inline]
fn slot(slots: &[Option<ItemStack>], i: usize) -> Option<ItemStack> {
    slots.get(i).copied().flatten()
}

fn smelt_result(
    slots: &[Option<ItemStack>],
    smelt: &impl Fn(ItemType) -> Option<ItemStack>,
) -> Option<ItemStack> {
    let input = slot(slots, SLOT_INPUT)?;
    if input.is_empty() {
        return None;
    }
    smelt(input.item)
}

fn can_smelt(slots: &[Option<ItemStack>], smelt: &impl Fn(ItemType) -> Option<ItemStack>) -> bool {
    match smelt_result(slots, smelt) {
        Some(r) => match slot(slots, SLOT_OUTPUT) {
            None => true,
            Some(o) => o.item == r.item && o.space_left() >= r.count,
        },
        None => false,
    }
}

fn produce(slots: &mut [Option<ItemStack>], result: ItemStack) {
    match &mut slots[SLOT_OUTPUT] {
        Some(o) => o.count += result.count,
        out @ None => *out = Some(result),
    }
    consume_one(slots, SLOT_INPUT);
}

fn consume_one(slots: &mut [Option<ItemStack>], i: usize) {
    if let Some(stack) = slots[i].as_mut() {
        stack.count -= 1;
        if stack.count == 0 {
            slots[i] = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn smelt(item: ItemType) -> Option<ItemStack> {
        match item {
            ItemType::RawIron => Some(ItemStack::new(ItemType::IronIngot, 1)),
            ItemType::RawCopper => Some(ItemStack::new(ItemType::CopperIngot, 1)),
            _ => None,
        }
    }

    fn run(f: &mut Furnace, slots: &mut [Option<ItemStack>; FURNACE_SLOTS], n: u32) {
        for _ in 0..n {
            f.tick(slots, smelt);
        }
    }

    fn stack(item: ItemType, n: u8) -> Option<ItemStack> {
        Some(ItemStack::new(item, n))
    }

    #[test]
    fn empty_furnace_does_nothing() {
        let mut f = Furnace::default();
        let mut slots = [None; FURNACE_SLOTS];
        assert!(
            !f.tick(&mut slots, smelt),
            "an idle furnace reports no change"
        );
        assert!(!f.is_lit());
        assert_eq!(f, Furnace::default());
    }

    #[test]
    fn smelts_one_item_in_cook_ticks() {
        let mut f = Furnace::default();
        let mut slots = [stack(ItemType::RawIron, 1), stack(ItemType::Coal, 1), None];
        // Lights on the first tick and begins cooking; burn_max mirrors the
        // fuel's row (derived, not pinned — the row is freely editable).
        assert!(f.tick(&mut slots, smelt));
        assert!(f.is_lit(), "furnace lights from the coal");
        assert_eq!(f.burn_max, ItemType::Coal.fuel_burn_ticks());
        // The remaining ticks complete the first (and only) smelt.
        run(&mut f, &mut slots, COOK_TICKS as u32 - 1);
        assert_eq!(slots[SLOT_OUTPUT], stack(ItemType::IronIngot, 1));
        assert!(
            slots[SLOT_INPUT].is_none(),
            "the single raw iron is consumed"
        );
        assert_eq!(f.cook_progress, 0);
    }

    #[test]
    fn one_coal_smelts_its_burn_worth_of_items() {
        let burn = ItemType::Coal.fuel_burn_ticks() as u32;
        let smelts = burn / COOK_TICKS as u32;
        let mut f = Furnace::default();
        let mut slots = [stack(ItemType::RawIron, 64), stack(ItemType::Coal, 1), None];
        // One coal burns `burn` ticks = `burn / COOK_TICKS` whole smelts.
        run(&mut f, &mut slots, burn);
        assert_eq!(slots[SLOT_OUTPUT].unwrap().count as u32, smelts);
        assert_eq!(slots[SLOT_INPUT].unwrap().count as u32, 64 - smelts);
        // Fuel is spent; one more tick puts the flame out.
        f.tick(&mut slots, smelt);
        assert!(!f.is_lit(), "the flame goes out once the coal is spent");
        assert!(slots[SLOT_FUEL].is_none());
    }

    #[test]
    fn flame_relights_from_a_second_coal() {
        let burn = ItemType::Coal.fuel_burn_ticks() as u32;
        let mut f = Furnace::default();
        let mut slots = [stack(ItemType::RawIron, 64), stack(ItemType::Coal, 2), None];
        // Past the first coal's burn: it relights from the second, staying lit.
        run(&mut f, &mut slots, burn + 10);
        assert!(f.is_lit(), "second coal keeps it burning");
        assert_eq!(slots[SLOT_FUEL], None, "both coal eventually consumed");
    }

    #[test]
    fn cook_regresses_when_input_runs_out_mid_smelt() {
        let mut f = Furnace::default();
        let mut slots = [stack(ItemType::RawIron, 1), stack(ItemType::Coal, 1), None];
        run(&mut f, &mut slots, 101); // light + cook to ~101
        assert!(f.cook_progress > 0);
        // Pull the input out: the cook bar now slides back even though the
        // fuel keeps burning.
        slots[SLOT_INPUT] = None;
        let progress = f.cook_progress;
        f.tick(&mut slots, smelt);
        assert!(
            f.cook_progress < progress,
            "cook bar eases back when nothing smelts"
        );
        assert!(f.is_lit(), "already-lit fuel keeps burning");
    }

    #[test]
    fn full_output_stops_smelting() {
        let mut f = Furnace::default();
        let mut slots = [
            stack(ItemType::RawIron, 5),
            stack(ItemType::Coal, 1),
            stack(ItemType::IronIngot, 64), // no room
        ];
        run(&mut f, &mut slots, 600);
        assert_eq!(
            slots[SLOT_OUTPUT].unwrap().count,
            64,
            "no ingot added when output is full"
        );
        assert_eq!(slots[SLOT_INPUT].unwrap().count, 5, "no input consumed");
        assert!(!f.is_lit(), "never lit: there was nothing it could smelt");
    }

    #[test]
    fn non_fuel_in_fuel_slot_does_not_burn() {
        let mut f = Furnace::default();
        let mut slots = [
            stack(ItemType::RawIron, 1),
            stack(ItemType::Dirt, 1), // not a fuel
            None,
        ];
        run(&mut f, &mut slots, 600);
        assert!(!f.is_lit());
        assert!(
            slots[SLOT_OUTPUT].is_none(),
            "nothing smelts without real fuel"
        );
        assert_eq!(
            slots[SLOT_FUEL],
            stack(ItemType::Dirt, 1),
            "non-fuel is left untouched"
        );
    }
}
