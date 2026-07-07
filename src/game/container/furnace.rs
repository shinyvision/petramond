//! The furnace's render view — the one genuinely furnace-specific piece of
//! the open session (the burn/cook gauges read the sibling machine state).
//! All slot behavior rides the generic [`SlotSpec`](crate::container::SlotSpec)
//! path in [`super::generic`].

use super::{ContainerMenu, ContainerTarget};
use crate::furnace::{SLOT_FUEL, SLOT_INPUT, SLOT_OUTPUT};
use crate::gui::FurnaceView;
use crate::world::World;

impl ContainerMenu {
    pub(in crate::game) fn open_furnace_view(&self, world: &World) -> Option<FurnaceView> {
        let ContainerTarget::Furnace(pos) = self.target else {
            return None;
        };
        let f = world.furnace_at(pos)?;
        let slot = |i: usize| {
            world
                .container_at(pos)
                .and_then(|c| c.slots.get(i).copied().flatten())
        };
        Some(FurnaceView {
            input: slot(SLOT_INPUT),
            fuel: slot(SLOT_FUEL),
            output: slot(SLOT_OUTPUT),
            cook01: f.cook_progress as f32 / crate::furnace::COOK_TICKS as f32,
            burn01: if f.burn_max == 0 {
                0.0
            } else {
                f.burn_remaining as f32 / f.burn_max as f32
            },
        })
    }
}
