use super::ContainerTarget;
use crate::crafting::CraftingStation;
use crate::gui::GuiKind;
use crate::inventory::Inventory;
use crate::item::ItemStack;
use crate::mathh::IVec3;
use crate::world::World;

/// The active container menu: transient station state plus the edit target the
/// open GUI mutates. Recipes stay on `ServerGame` because machine processing
/// consumes the same catalog.
#[derive(Default)]
pub(crate) struct ContainerMenu {
    /// What the open GUI is currently editing (a block-entity or crafting session).
    pub(super) target: ContainerTarget,
    /// The real transient result of an explicit CRAFT action. It is returned
    /// to inventory/drop on close, never recomputed as a preview.
    pub(super) craft_output: Option<ItemStack>,
    /// The furniture workbench's single input block while its screen is open. Transient
    /// (returned to the inventory on close), so the workbench owns
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

    /// The transient player-crafting output slot.
    #[inline]
    pub(crate) fn craft_output(&self) -> Option<ItemStack> {
        self.craft_output
    }

    /// Station-owned stacks that a crash-safe player snapshot must project
    /// back into inventory even though the live menu remains open.
    pub(crate) fn unpersisted_items(&self) -> [Option<ItemStack>; 2] {
        [self.craft_output, self.workbench_input]
    }

    pub(crate) fn crafting_station(&self) -> Option<CraftingStation> {
        match self.target.kind() {
            Some(GuiKind::Inventory) => Some(CraftingStation::Inventory),
            Some(GuiKind::CraftingTable) => Some(CraftingStation::CraftingTable),
            _ => None,
        }
    }

    /// Change the admitted player-crafting station. The caller closes an old
    /// session before replacement; preserving the slot here is a final no-loss
    /// guard for direct/test callers.
    pub(crate) fn open_crafting(&mut self, station: CraftingStation) {
        let kind = match station {
            CraftingStation::Inventory => GuiKind::Inventory,
            CraftingStation::CraftingTable => GuiKind::CraftingTable,
        };
        self.target = ContainerTarget::Gui { kind, pos: None };
    }

    /// Begin a furnace-screen session at `pos`: remember which furnace the GUI
    /// reads and edits. Defensively creates an empty entity if the block lacks one
    /// (placement always inserts one, so this is belt-and-braces).
    pub(crate) fn open_furnace_screen(&mut self, world: &mut World, pos: IVec3) {
        if world.furnace_at(pos).is_none() {
            world.insert_furnace(pos, crate::facing::Facing::default());
        }
        self.target = ContainerTarget::Gui {
            kind: GuiKind::Furnace,
            pos: Some(pos),
        };
    }

    /// End the furnace-screen session. The furnace keeps its block-entity contents.
    pub(crate) fn close_furnace(&mut self) {
        self.close_kind(|kind| kind == GuiKind::Furnace);
    }

    /// Begin a chest-screen session at `pos`: remember which chest the GUI reads and
    /// edits. Defensively creates an empty chest if the block lacks one (placement
    /// always inserts one, so this is belt-and-braces).
    pub(crate) fn open_chest_screen(&mut self, world: &mut World, pos: IVec3) {
        if world.container_at(pos).is_none() {
            world.insert_chest(pos, crate::facing::Facing::default());
        }
        self.target = ContainerTarget::Gui {
            kind: GuiKind::Chest,
            pos: Some(pos),
        };
    }

    /// End the chest-screen session. The chest keeps its block-entity contents.
    pub(crate) fn close_chest(&mut self) {
        self.close_kind(|kind| kind == GuiKind::Chest);
    }

    /// Begin a furniture-workbench session: the input slot starts empty.
    pub(crate) fn open_workbench(&mut self) {
        self.target = ContainerTarget::Gui {
            kind: GuiKind::FurnitureWorkbench,
            pos: None,
        };
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
        kind: GuiKind,
        pos: Option<crate::mathh::IVec3>,
    ) {
        let pos = pos.map(|p| world.container_anchor(p));
        let specs = crate::gui::documents::container_slot_specs(kind);
        if let (Some(p), false) = (pos, specs.is_empty()) {
            world.ensure_container(p, specs.len());
        }
        self.target = ContainerTarget::Gui { kind, pos };
    }

    /// End the mod GUI session (the state map is cleared by `Game`'s close
    /// funnel, which knows the world).
    pub(crate) fn close_mod_gui(&mut self) {
        self.close_kind(|kind| kind.is_mod());
    }

    /// End the workbench session: return the input block to the inventory
    /// (overflow goes through `overflow`), then drop the transient target.
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
        self.close_kind(|kind| kind == GuiKind::FurnitureWorkbench);
    }

    /// Close player crafting: return its real output to inventory (overflow is
    /// thrown through `overflow`) and drop the target.
    pub(crate) fn close_crafting(
        &mut self,
        inv: &mut Inventory,
        mut overflow: impl FnMut(ItemStack),
    ) {
        if let Some(stack) = self.craft_output.take() {
            if let Some(leftover) = inv.add(stack) {
                overflow(leftover);
            }
        }
        self.close_kind(|kind| kind == GuiKind::Inventory || kind == GuiKind::CraftingTable);
    }

    /// Drop the target if the open session's kind matches.
    fn close_kind(&mut self, matches: impl Fn(GuiKind) -> bool) {
        if self.target.kind().is_some_and(matches) {
            self.target = ContainerTarget::None;
        }
    }
}
