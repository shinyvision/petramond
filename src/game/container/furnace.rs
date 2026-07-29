//! Gauge state published by an open container's block entity.
//!
//! A machine's slots ride the generic [`SlotSpec`](crate::container::SlotSpec)
//! path in [`super::generic`] like every other container's. What a machine
//! additionally has is READINGS — the furnace's cook arrow and burn flame —
//! and those ship as ordinary named GUI-state values, the same channel a
//! pack's machine publishes through. The document binds a `gauge` node to the
//! key; neither the wire nor the GUI vocabulary knows a furnace exists.

use super::{ContainerMenu, ContainerTarget};
use crate::world::World;

impl ContainerMenu {
    /// The named gauge readings the open container's block entity publishes
    /// this tick, or empty when it publishes none (a plain chest, a pack
    /// container whose state is its own).
    ///
    /// Engine machines answer here rather than earning a wire variant of
    /// their own; a pack machine writes the same keys through its GUI state.
    pub(crate) fn open_gauges(&self, world: &World) -> Vec<(String, f32)> {
        let ContainerTarget::Gui { pos: Some(pos), .. } = self.target else {
            return Vec::new();
        };
        let Some(f) = world.furnace_at(pos) else {
            return Vec::new();
        };
        vec![
            (
                "cook01".to_string(),
                f.cook_progress as f32 / crate::furnace::COOK_TICKS as f32,
            ),
            (
                "burn01".to_string(),
                if f.burn_max == 0 {
                    0.0
                } else {
                    f.burn_remaining as f32 / f.burn_max as f32
                },
            ),
        ]
    }
}
