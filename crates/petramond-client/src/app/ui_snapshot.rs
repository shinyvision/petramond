use crate::app::AppScreen;
use crate::game::Game;
use petramond::gui::UiSnapshot;
use petramond_world::gui_state::MenuSlot;
use petramond_world::inventory::{place_cursor_count, plan_drag_distribution, slot_capacity};
use petramond_world::item::ItemStack;

pub(super) fn build(
    game: Option<&Game>,
    screen: AppScreen,
    cursor_px: (f32, f32),
    drag_preview: Option<(&[MenuSlot], petramond_ui::PointerButton)>,
) -> UiSnapshot {
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
    snapshot.craft_output = menu.craft_output;
    snapshot.cursor = inv.cursor().copied();
    snapshot.container = menu.container;
    snapshot.gui_state = menu.gui_state;
    snapshot.health = game.player_health();
    snapshot.effects = game.player_effect_icons();

    for (i, slot) in snapshot.slots.iter_mut().enumerate() {
        *slot = inv.slot(i).copied();
    }
    if let Some((slots, button)) = drag_preview {
        apply_menu_drag_preview(&mut snapshot, slots, button);
    }
    snapshot
}

/// Overlay the active pointer gesture onto the immutable render snapshot so
/// every newly hit slot responds in the frame it is hit. Release replaces
/// this ephemeral overlay with the identical rollback-backed menu prediction.
pub(super) fn apply_menu_drag_preview(
    snapshot: &mut UiSnapshot,
    slots: &[MenuSlot],
    button: petramond_ui::PointerButton,
) {
    let Some(held) = snapshot.cursor else {
        return;
    };
    let specs = petramond::gui::documents::container_slot_specs(snapshot.kind);
    let plan = plan_drag_distribution(
        slots,
        held.count,
        button == petramond_ui::PointerButton::Secondary,
        |slot| preview_capacity(snapshot, &specs, slot, &held),
    );
    let mut cursor = Some(held);
    for (slot, wanted) in plan {
        preview_place(snapshot, &specs, &mut cursor, slot, wanted);
    }
    snapshot.cursor = cursor;
}

fn preview_capacity(
    snapshot: &UiSnapshot,
    specs: &[petramond_world::container::SlotSpec],
    slot: MenuSlot,
    held: &ItemStack,
) -> u8 {
    match slot {
        MenuSlot::Inventory(i) => snapshot
            .slots
            .get(i)
            .map(|cell| slot_capacity(cell, held))
            .unwrap_or(0),
        // The same question the committed prediction and the server both ask
        // (`container::slot_admits`). Asking a THIRD one here means the drag
        // preview shows a split the click that follows it will not perform.
        MenuSlot::Container(i) if petramond_world::container::slot_admits(specs, i, Some(held.item)) => {
            snapshot
                .container
                .as_ref()
                .and_then(|container| container.slots.get(i))
                .map(|cell| slot_capacity(cell, held))
                .unwrap_or(0)
        }
        MenuSlot::CraftResult | MenuSlot::Container(_) | MenuSlot::Widget(_) => 0,
    }
}

fn preview_place(
    snapshot: &mut UiSnapshot,
    specs: &[petramond_world::container::SlotSpec],
    cursor: &mut Option<ItemStack>,
    slot: MenuSlot,
    wanted: u8,
) {
    if let MenuSlot::Inventory(i) = slot {
        if let Some(cell) = snapshot.slots.get_mut(i) {
            place_cursor_count(cursor, cell, wanted);
        }
        return;
    }

    let cell = match slot {
        MenuSlot::Container(i)
            if petramond_world::container::slot_admits(specs, i, cursor.as_ref().map(|c| c.item)) =>
        {
            snapshot
                .container
                .as_mut()
                .and_then(|container| container.slots.get_mut(i))
        }
        MenuSlot::Inventory(_)
        | MenuSlot::CraftResult
        | MenuSlot::Container(_)
        | MenuSlot::Widget(_) => None,
    };
    if let Some(cell) = cell {
        place_cursor_count(cursor, cell, wanted);
    }
}
