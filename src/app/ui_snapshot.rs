use crate::app::AppScreen;
use crate::game::Game;
use crate::gui::UiSnapshot;
use crate::item::{ItemStack, ItemType};

pub(super) fn build(
    game: &Game,
    screen: AppScreen,
    screen_size: (u32, u32),
    cursor_px: (f32, f32),
) -> UiSnapshot {
    let menu = game.menu_read_model();
    let inv = menu.inventory;
    let craft = menu.craft;
    let mut snapshot = UiSnapshot {
        open: screen.ui_open(),
        kind: screen.gui_kind(),
        screen: screen_size,
        cursor_px,
        active: inv.active_slot(),
        result: craft.result().copied().map(stack_tuple),
        cursor: inv.cursor().copied().map(stack_tuple),
        furnace: menu.furnace,
        chest: menu.chest,
        workbench: menu.workbench,
        ..Default::default()
    };

    for (i, slot) in snapshot.slots.iter_mut().enumerate() {
        *slot = inv.slot(i).copied().map(stack_tuple);
    }
    for (slot, cell) in snapshot.craft.iter_mut().zip(craft.cells()) {
        *slot = cell.as_ref().copied().map(stack_tuple);
    }

    snapshot
}

fn stack_tuple(stack: ItemStack) -> (ItemType, u8) {
    (stack.item, stack.count)
}
