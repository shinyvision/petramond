use crate::app::AppScreen;
use crate::game::Game;
use crate::gui::{ShellUiSnapshot, UiSnapshot};
use crate::item::{ItemStack, ItemType};

pub(super) fn build(
    game: Option<&Game>,
    screen: AppScreen,
    screen_size: (u32, u32),
    cursor_px: (f32, f32),
    shell: ShellUiSnapshot,
) -> UiSnapshot {
    let mut snapshot = UiSnapshot {
        open: screen.ui_open(),
        kind: screen.gui_kind(),
        screen: screen_size,
        cursor_px,
        shell,
        ..Default::default()
    };

    let Some(game) = game else {
        return snapshot;
    };

    let menu = game.menu_read_model();
    let inv = menu.inventory;
    let craft = menu.craft;
    snapshot.active = inv.active_slot();
    snapshot.result = craft.result().copied().map(stack_tuple);
    snapshot.cursor = inv.cursor().copied().map(stack_tuple);
    snapshot.furnace = menu.furnace;
    snapshot.chest = menu.chest;
    snapshot.workbench = menu.workbench;
    snapshot.health = game.player_health();

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
