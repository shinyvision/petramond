use crate::app::AppScreen;
use crate::game::Game;
use crate::gui::UiSnapshot;
use crate::item::{ItemStack, ItemType};

pub(super) fn build(game: Option<&Game>, screen: AppScreen, cursor_px: (f32, f32)) -> UiSnapshot {
    let mut snapshot = UiSnapshot {
        open: screen.ui_open(),
        kind: screen.gui_kind(),
        cursor_px,
        ..Default::default()
    };

    let Some(game) = game else {
        return snapshot;
    };

    let menu = game.menu_read_model();
    let inv = menu.inventory;
    snapshot.active = inv.active_slot();
    snapshot.craft_output = menu.craft_output.map(stack_tuple);
    snapshot.cursor = inv.cursor().copied().map(stack_tuple);
    snapshot.furnace = menu.furnace;
    snapshot.chest = menu.chest;
    snapshot.workbench = menu.workbench;
    snapshot.container = menu.container;
    snapshot.gui_state = menu.gui_state;
    snapshot.health = game.player_health();
    snapshot.effects = game.player_effect_icons();

    for (i, slot) in snapshot.slots.iter_mut().enumerate() {
        *slot = inv.slot(i).copied().map(stack_tuple);
    }
    snapshot
}

fn stack_tuple(stack: ItemStack) -> (ItemType, u8) {
    (stack.item, stack.count)
}
