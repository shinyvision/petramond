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
use petramond::gui::documents::{slot_tip_keys, SHOW_SLOT_TIP, SLOT_TIP_LINES, SLOT_TIP_SPANS};
use petramond::gui::Role;
use petramond_world::inventory::HOTBAR_LEN;
use petramond_world::item::ItemStack;
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
    let stack = stack.filter(|stack| stack.item != petramond_world::item::ItemType::Air && stack.count > 0);
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

/// Publish the BOUND slot-tip keys: a machine may publish per-slot tooltip
/// LINES under the gui-state key `slot{index}:tip` — lines separated by
/// newline, each line up to [`SLOT_TIP_SPANS`] spans separated by tab, each
/// span a `palette|text` pair (the palette half naming a theme palette
/// entry, `bind.palette`). Spans colour one word of a line without
/// painting the rest. Hovering that container slot while it holds NO stack
/// floats the lines at the pointer; the item tip owns a hovered stack, so
/// the two never show together. Runs after the gui-state copy, so the tip
/// value is already in `state`.
///
/// Newline, tab and `|` are RESERVED separators with no escape: a publisher
/// must strip them from its own text, or the line splits where it did not
/// mean to. Nothing sanitises them here — that would hide the publisher's
/// bug behind a tooltip that quietly reads wrong.
pub(super) fn populate_slot_tip(
    game: &Game,
    hover_slot: Option<&(String, u32)>,
    state: &mut UiState,
) {
    let tip: Option<String> = if game.cursor_has_stack() {
        None
    } else {
        hover_slot
            .filter(|(role, _)| matches!(Role::from_key(role), Some(Role::Container)))
            .filter(|(role, index)| {
                hovered_stack(game, role, *index as usize)
                    .filter(|s| {
                        s.item != petramond_world::item::ItemType::Air && s.count > 0
                    })
                    .is_none()
            })
            .and_then(|(_, index)| state.get_str(&format!("slot{index}:tip")))
            .filter(|s| !s.is_empty())
            .map(|s| s.to_owned())
    };
    let lines: Vec<&str> = tip
        .as_deref()
        .map(|t| t.lines().take(SLOT_TIP_LINES).collect())
        .unwrap_or_default();
    state.set(SHOW_SLOT_TIP, UiValue::Bool(!lines.is_empty()));
    for i in 0..SLOT_TIP_LINES {
        let spans: Vec<&str> = lines
            .get(i)
            .map(|l| l.split('\t').take(SLOT_TIP_SPANS).collect())
            .unwrap_or_default();
        for j in 0..SLOT_TIP_SPANS {
            let (pal, text) = spans
                .get(j)
                .map(|s| s.split_once('|').unwrap_or(("", *s)))
                .unwrap_or(("", ""));
            let (text_key, palette_key) = slot_tip_keys(i, j);
            state.set(text_key, UiValue::Str(text.to_owned()));
            state.set(palette_key, UiValue::Str(pal.to_owned()));
        }
    }
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
