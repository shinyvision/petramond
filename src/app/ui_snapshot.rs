use crate::app::AppScreen;
use crate::game::Game;
use crate::gui::{FurnaceHit, MenuSlot, UiSnapshot};
use crate::inventory::{place_cursor_count, plan_drag_distribution, slot_capacity};
use crate::item::{ItemStack, ItemType};

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
    snapshot.craft_output = menu.craft_output.map(stack_tuple);
    snapshot.cursor = inv.cursor().copied().map(stack_tuple);
    snapshot.furnace = menu.furnace;
    snapshot.chest = menu.chest;
    snapshot.container = menu.container;
    snapshot.gui_state = menu.gui_state;
    snapshot.health = game.player_health();
    snapshot.effects = game.player_effect_icons();

    for (i, slot) in snapshot.slots.iter_mut().enumerate() {
        *slot = inv.slot(i).copied().map(stack_tuple);
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
    let Some((item, held_count)) = snapshot.cursor else {
        return;
    };
    let specs = crate::gui::documents::container_slot_specs(snapshot.kind);
    let plan = plan_drag_distribution(
        slots,
        held_count,
        button == petramond_ui::PointerButton::Secondary,
        |slot| preview_capacity(snapshot, &specs, slot, item),
    );
    let mut cursor = Some(ItemStack::new(item, held_count));
    for (slot, wanted) in plan {
        preview_place(snapshot, &specs, &mut cursor, slot, wanted);
    }
    snapshot.cursor = cursor.map(stack_tuple);
}

fn preview_capacity(
    snapshot: &UiSnapshot,
    specs: &[crate::container::SlotSpec],
    slot: MenuSlot,
    item: ItemType,
) -> u8 {
    match slot {
        MenuSlot::Inventory(i) => snapshot
            .slots
            .get(i)
            .map(|cell| slot_capacity(&cell.map(stack_from_tuple), item))
            .unwrap_or(0),
        MenuSlot::Furnace(hit) => snapshot
            .furnace
            .as_ref()
            .map(|furnace| match hit {
                FurnaceHit::Input => slot_capacity(&furnace.input, item),
                FurnaceHit::Fuel => slot_capacity(&furnace.fuel, item),
                FurnaceHit::Output => 0,
            })
            .unwrap_or(0),
        MenuSlot::Chest(i) => snapshot
            .chest
            .as_ref()
            .and_then(|chest| chest.slots.get(i))
            .map(|cell| slot_capacity(cell, item))
            .unwrap_or(0),
        MenuSlot::Container(i) if specs.get(i).is_some_and(|spec| !spec.take_only) => snapshot
            .container
            .as_ref()
            .and_then(|container| container.slots.get(i))
            .map(|cell| slot_capacity(cell, item))
            .unwrap_or(0),
        MenuSlot::CraftResult | MenuSlot::Container(_) | MenuSlot::Widget(_) => 0,
    }
}

fn preview_place(
    snapshot: &mut UiSnapshot,
    specs: &[crate::container::SlotSpec],
    cursor: &mut Option<ItemStack>,
    slot: MenuSlot,
    wanted: u8,
) {
    if let MenuSlot::Inventory(i) = slot {
        let Some(cell) = snapshot.slots.get_mut(i) else {
            return;
        };
        let mut stack = cell.map(stack_from_tuple);
        place_cursor_count(cursor, &mut stack, wanted);
        *cell = stack.map(stack_tuple);
        return;
    }

    let cell = match slot {
        MenuSlot::Furnace(FurnaceHit::Input) => {
            snapshot.furnace.as_mut().map(|furnace| &mut furnace.input)
        }
        MenuSlot::Furnace(FurnaceHit::Fuel) => {
            snapshot.furnace.as_mut().map(|furnace| &mut furnace.fuel)
        }
        MenuSlot::Chest(i) => snapshot
            .chest
            .as_mut()
            .and_then(|chest| chest.slots.get_mut(i)),
        MenuSlot::Container(i) if specs.get(i).is_some_and(|spec| !spec.take_only) => snapshot
            .container
            .as_mut()
            .and_then(|container| container.slots.get_mut(i)),
        MenuSlot::Inventory(_)
        | MenuSlot::CraftResult
        | MenuSlot::Furnace(FurnaceHit::Output)
        | MenuSlot::Container(_)
        | MenuSlot::Widget(_) => None,
    };
    if let Some(cell) = cell {
        place_cursor_count(cursor, cell, wanted);
    }
}

fn stack_tuple(stack: ItemStack) -> (ItemType, u8) {
    (stack.item, stack.count)
}

fn stack_from_tuple((item, count): (ItemType, u8)) -> ItemStack {
    ItemStack::new(item, count)
}
