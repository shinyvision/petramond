//! The slot tooltip: hovering a filled slot in any menu floats the item's
//! display name (and its optional `info` line) at the pointer.
//!
//! The tooltip itself is an ordinary document node bound to the keys
//! [`populate`] publishes; this module's only job is answering "which stack is
//! under the cursor" from the previous frame's `FrameOutput::hover_slot` —
//! the same one-frame-old hover contract as the recipe browser's
//! (`UiRuntime` resolves hover after input, so a populate pass always reads
//! the last solved frame).

use crate::game::Game;
use crate::gui::Role;
use crate::inventory::HOTBAR_LEN;
use crate::item::ItemStack;
use petramond_ui::{UiState, UiValue};

/// Publish the item-tip keys for the slot hovered on the last solved frame.
/// Runs for every menu kind: keys a document does not bind are inert, so a
/// pack document opts into the same tooltip by adding the node itself.
pub(super) fn populate(game: &Game, hover_slot: Option<&(String, u32)>, state: &mut UiState) {
    // A tooltip that follows the cursor would fight the held stack for the
    // same pixels, so a drag suppresses it (the recipe tip's rule).
    let stack = if game.cursor_has_stack() {
        None
    } else {
        hover_slot.and_then(|(role, index)| hovered_stack(game, role, *index as usize))
    };
    let stack = stack.filter(|stack| stack.item != crate::item::ItemType::Air && stack.count > 0);
    state.set("show_item_tip", UiValue::Bool(stack.is_some()));
    state.set(
        "item_tip_name",
        UiValue::Str(stack.map(|stack| stack.item.name().to_owned()).unwrap_or_default()),
    );
    state.set(
        "item_tip_info",
        UiValue::Str(
            stack
                .and_then(|stack| stack.item.info())
                .unwrap_or_default()
                .to_owned(),
        ),
    );
    state.set(
        "item_tip_has_info",
        UiValue::Bool(stack.and_then(|stack| stack.item.info()).is_some()),
    );
}

/// The stack a hovered document slot cell shows, by role — the same
/// role→model mapping the renderer's `slot_item` uses to draw slot icons.
fn hovered_stack(game: &Game, role: &str, index: usize) -> Option<ItemStack> {
    let menu = game.menu_read_model();
    match Role::from_key(role)? {
        Role::Hotbar => menu.inventory.slot(index).copied(),
        Role::PlayerInv => menu.inventory.slot(HOTBAR_LEN + index).copied(),
        Role::CraftResult => menu.craft_output,
        Role::Container => menu
            .container
            .as_ref()
            .and_then(|container| container.slots.get(index).copied().flatten()),
        Role::Generic | Role::Other => None,
    }
}
