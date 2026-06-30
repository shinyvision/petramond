use crate::inventory::Inventory;
use crate::item::{ItemStack, ItemTag, ItemType};
pub const COOK_TICKS: u16 = 600;
const COOK_REGRESS: u16 = 2;
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
    #[inline]
    pub fn to_u8(self) -> u8 {
        self as u8
    }
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
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FillSlot {
    Input,
    Fuel,
}
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Furnace {
    pub input: Option<ItemStack>,
    pub fuel: Option<ItemStack>,
    pub output: Option<ItemStack>,
    pub cook_progress: u16,
    pub burn_remaining: u16,
    pub burn_max: u16,
    pub facing: Facing,
}

impl Furnace {
    #[inline]
    pub fn is_lit(&self) -> bool {
        self.burn_remaining > 0
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.input.is_none() && self.fuel.is_none() && self.output.is_none()
    }
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
    pub fn fill_slot_for(item: ItemType) -> Option<FillSlot> {
        if item.has_tag(ItemTag::Fuel) {
            Some(FillSlot::Fuel)
        } else if item.has_tag(ItemTag::Smeltable) {
            Some(FillSlot::Input)
        } else {
            None
        }
    }
    pub fn shift_in(&mut self, role: FillSlot, src: &mut Option<ItemStack>) {
        let dst = match role {
            FillSlot::Input => &mut self.input,
            FillSlot::Fuel => &mut self.fuel,
        };
        merge_stack(src, dst);
    }
    #[inline]
    pub fn input_slot(&mut self) -> &mut Option<ItemStack> {
        &mut self.input
    }
    #[inline]
    pub fn fuel_slot(&mut self) -> &mut Option<ItemStack> {
        &mut self.fuel
    }
    pub fn shift_output_into(&mut self, inv: &mut Inventory) {
        inv.pull_from(&mut self.output);
    }
    fn smelt_result(&self, smelt: &impl Fn(ItemType) -> Option<ItemStack>) -> Option<ItemStack> {
        let input = self.input?;
        if input.is_empty() {
            return None;
        }
        smelt(input.item)
    }
    fn output_accepts(&self, result: ItemStack) -> bool {
        match self.output {
            None => true,
            Some(o) => o.item == result.item && o.space_left() >= result.count,
        }
    }
    fn can_smelt(&self, smelt: &impl Fn(ItemType) -> Option<ItemStack>) -> bool {
        match self.smelt_result(smelt) {
            Some(r) => self.output_accepts(r),
            None => false,
        }
    }
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
    fn consume_fuel(&mut self) {
        if let Some(fuel) = self.fuel.as_mut() {
            fuel.count -= 1;
            if fuel.count == 0 {
                self.fuel = None;
            }
        }
    }
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
        assert!(
            f.cook_progress < progress,
            "cook bar eases back when nothing smelts"
        );
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
        assert_eq!(
            f.output.unwrap().count,
            64,
            "no ingot added when output is full"
        );
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
        assert_eq!(
            f.fuel,
            stack(ItemType::Dirt, 1),
            "non-fuel is left untouched"
        );
    }
}
