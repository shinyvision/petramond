use super::ContainerTarget;
use crate::crafting::{CraftGrid, Recipes};
use crate::inventory::Inventory;
use crate::item::ItemStack;
use crate::mathh::IVec3;
use crate::world::World;

/// The active container menu: the crafting grid (2×2 in the inventory, 3×3 at a
/// table) plus the edit target the open GUI mutates.
///
/// Recipes are NOT owned here — the furnace *smelting* tick on `Game` also needs
/// them, so they stay on `Game` and are borrowed into the craft methods per call.
/// That keeps the smelt tick free of a self-referential borrow.
#[derive(Default)]
pub(crate) struct ContainerMenu {
    /// What the open GUI is currently editing (a block-entity or the craft grid).
    pub(super) target: ContainerTarget,
    /// The active crafting grid + its cached result. Empty whenever no crafting
    /// screen is open.
    pub(super) craft: CraftGrid,
    /// The furniture workbench's single input block while its screen is open. Transient
    /// (returned to the inventory on close, like the craft grid), so the workbench owns
    /// no block-entity. `None` whenever no workbench screen is open.
    pub(super) workbench_input: Option<ItemStack>,
}

impl ContainerMenu {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// What the open GUI is editing (read by the lid animation + render views).
    #[inline]
    pub(crate) fn target(&self) -> ContainerTarget {
        self.target
    }

    /// The active crafting grid (for the UI to read cells + result preview).
    #[inline]
    pub(crate) fn craft_grid(&self) -> &CraftGrid {
        &self.craft
    }

    /// Mutable grid access for tests that stage cells directly instead of
    /// walking the click router.
    #[cfg(test)]
    pub(crate) fn craft_grid_mut(&mut self) -> &mut CraftGrid {
        &mut self.craft
    }

    /// Configure the crafting grid for a screen of `cols×cols` (2 = inventory,
    /// 3 = table) and clear it. Called when a crafting screen opens.
    pub(crate) fn open_crafting(&mut self, cols: usize, recipes: &Recipes) {
        self.target = if cols >= 3 {
            ContainerTarget::Table
        } else {
            ContainerTarget::Inventory
        };
        self.craft.reset(cols);
        self.craft.recompute(recipes);
    }

    /// Begin a furnace-screen session at `pos`: remember which furnace the GUI
    /// reads and edits. Defensively creates an empty entity if the block lacks one
    /// (placement always inserts one, so this is belt-and-braces).
    pub(crate) fn open_furnace_screen(&mut self, world: &mut World, pos: IVec3) {
        if world.furnace_at(pos).is_none() {
            world.insert_furnace(pos, crate::facing::Facing::default());
        }
        self.target = ContainerTarget::Furnace(pos);
    }

    /// End the furnace-screen session. The furnace keeps its contents (unlike the
    /// crafting grid, which empties back into the inventory on close).
    pub(crate) fn close_furnace(&mut self) {
        if matches!(self.target, ContainerTarget::Furnace(_)) {
            self.target = ContainerTarget::None;
        }
    }

    /// Begin a chest-screen session at `pos`: remember which chest the GUI reads and
    /// edits. Defensively creates an empty chest if the block lacks one (placement
    /// always inserts one, so this is belt-and-braces).
    pub(crate) fn open_chest_screen(&mut self, world: &mut World, pos: IVec3) {
        if world.container_at(pos).is_none() {
            world.insert_chest(pos, crate::facing::Facing::default());
        }
        self.target = ContainerTarget::Chest(pos);
    }

    /// End the chest-screen session. The chest keeps its contents (like the furnace,
    /// unlike the crafting grid which empties back into the inventory on close).
    pub(crate) fn close_chest(&mut self) {
        if matches!(self.target, ContainerTarget::Chest(_)) {
            self.target = ContainerTarget::None;
        }
    }

    /// Begin a furniture-workbench session: the input slot starts empty.
    pub(crate) fn open_workbench(&mut self) {
        self.target = ContainerTarget::FurnitureWorkbench;
        self.workbench_input = None;
    }

    /// Begin a mod GUI session for `kind`, opened from `pos` (`None` for a
    /// programmatic open). The state map lives on the world; `Game`'s open
    /// funnel clears it around this call.
    ///
    /// A slot-bearing kind (its document declares `container` slots) gets its
    /// backing storage here: `pos` is canonicalized to the block's container
    /// anchor (multi-cell model blocks share ONE container at the group base,
    /// whichever cell was clicked) and a container sized to the document is
    /// created — or grown, never shrunk — at it.
    pub(crate) fn open_mod_gui(
        &mut self,
        world: &mut World,
        kind: crate::gui::GuiKind,
        pos: Option<crate::mathh::IVec3>,
    ) {
        let pos = pos.map(|p| world.container_anchor(p));
        let specs = crate::gui::documents::container_slot_specs(kind);
        if let (Some(p), false) = (pos, specs.is_empty()) {
            world.ensure_container(p, specs.len());
        }
        self.target = ContainerTarget::ModGui { kind, pos };
    }

    /// End the mod GUI session (the state map is cleared by `Game`'s close
    /// funnel, which knows the world).
    pub(crate) fn close_mod_gui(&mut self) {
        if matches!(self.target, ContainerTarget::ModGui { .. }) {
            self.target = ContainerTarget::None;
        }
    }

    /// End the workbench session: return the input block to the inventory (overflow
    /// thrown into the world via `overflow`), then drop the target — like the crafting
    /// grid, the workbench is a station that holds nothing once closed.
    pub(crate) fn close_workbench(
        &mut self,
        inv: &mut Inventory,
        mut overflow: impl FnMut(ItemStack),
    ) {
        if let Some(stack) = self.workbench_input.take() {
            if let Some(leftover) = inv.add(stack) {
                overflow(leftover);
            }
        }
        if matches!(self.target, ContainerTarget::FurnitureWorkbench) {
            self.target = ContainerTarget::None;
        }
    }

    /// Close the crafting grid: return every input item to the inventory (any
    /// overflow is thrown into the world via `overflow`), then clear the result and
    /// drop the craft target.
    pub(crate) fn close_crafting(
        &mut self,
        inv: &mut Inventory,
        recipes: &Recipes,
        mut overflow: impl FnMut(ItemStack),
    ) {
        for i in 0..self.craft.capacity() {
            if let Some(stack) = self.craft.take_cell(i) {
                if let Some(leftover) = inv.add(stack) {
                    overflow(leftover);
                }
            }
        }
        self.craft.recompute(recipes);
        if matches!(
            self.target,
            ContainerTarget::Inventory | ContainerTarget::Table
        ) {
            self.target = ContainerTarget::None;
        }
    }
}
