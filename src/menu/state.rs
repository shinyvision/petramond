use super::{ContainerTarget, MenuAnchor};
use crate::world::ServerWorld;
use petramond_math::math::IVec3;
use petramond_world::crafting::CraftingStation;
use petramond_world::gui_state::GuiKind;
use petramond_world::inventory::Inventory;
use petramond_world::item::ItemStack;

#[derive(Default)]
pub struct ContainerMenu {
    pub(super) target: ContainerTarget,
    pub(super) craft_output: Option<ItemStack>,
}

impl ContainerMenu {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub fn target(&self) -> ContainerTarget {
        self.target
    }

    #[inline]
    pub fn craft_output(&self) -> Option<ItemStack> {
        self.craft_output
    }

    pub fn unpersisted_items(&self) -> [Option<ItemStack>; 1] {
        [self.craft_output]
    }

    pub fn crafting_station(&self) -> Option<CraftingStation> {
        self.target.kind().and_then(CraftingStation::of_kind)
    }

    pub fn open_crafting(&mut self, station: CraftingStation) {
        self.target = ContainerTarget::Gui {
            kind: station.gui_kind(),
            anchor: None,
        };
    }

    pub fn open_furnace_screen(&mut self, world: &mut ServerWorld, pos: IVec3) {
        if world.furnace_at(pos).is_none() {
            world.insert_furnace(pos, petramond_math::facing::Facing::default());
        }
        self.target = ContainerTarget::Gui {
            kind: GuiKind::Furnace,
            anchor: Some(MenuAnchor::Block(pos)),
        };
    }

    pub fn close_furnace(&mut self) {
        self.close_kind(|kind| kind == GuiKind::Furnace);
    }

    pub fn open_chest_screen(&mut self, world: &mut ServerWorld, pos: IVec3) {
        world.stock_loot(pos);
        world.ensure_container(pos, crate::world::chest::CHEST_SLOTS);
        self.target = ContainerTarget::Gui {
            kind: GuiKind::Chest,
            anchor: Some(MenuAnchor::Block(pos)),
        };
    }

    pub fn close_chest(&mut self) {
        self.close_kind(|kind| kind == GuiKind::Chest);
    }

    pub fn open_document_gui(
        &mut self,
        world: &mut ServerWorld,
        kind: GuiKind,
        anchor: Option<MenuAnchor>,
    ) {
        let anchor = anchor.map(|anchor| match anchor {
            MenuAnchor::Block(p) => MenuAnchor::Block(world.container_anchor(p)),
            mob @ MenuAnchor::Mob(_) => mob,
        });
        if let Some(MenuAnchor::Block(p)) = anchor {
            let specs = super::slot_specs_for_kind(kind);
            if !specs.is_empty() {
                world.ensure_container(p, specs.len());
            }
        }
        self.target = ContainerTarget::Gui { kind, anchor };
    }

    pub fn close_document_gui(&mut self) {
        self.close_kind(|kind| kind.is_registered() || kind == GuiKind::Creative);
    }

    pub fn close_crafting(&mut self, inv: &mut Inventory, mut overflow: impl FnMut(ItemStack)) {
        if let Some(stack) = self.craft_output.take() {
            if let Some(leftover) = inv.add(stack) {
                overflow(leftover);
            }
        }
        self.close_kind(|kind| CraftingStation::of_kind(kind).is_some());
    }

    fn close_kind(&mut self, matches: impl Fn(GuiKind) -> bool) {
        if self.target.kind().is_some_and(matches) {
            self.target = ContainerTarget::None;
        }
    }
}
