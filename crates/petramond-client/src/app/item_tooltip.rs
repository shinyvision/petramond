use crate::game::Game;
use petramond::gui::documents::{slot_tip_keys, SHOW_SLOT_TIP, SLOT_TIP_LINES, SLOT_TIP_SPANS};
use petramond::gui::Role;
use petramond_ui::{UiState, UiValue};
use petramond_world::inventory::HOTBAR_LEN;
use petramond_world::item::ItemStack;

pub(super) fn populate(
    game: &Game,
    hover_slot: Option<&(String, u32)>,
    state: &mut UiState,
) -> Vec<(String, std::path::PathBuf)> {
    let stack = if game.cursor_has_stack() {
        None
    } else {
        hover_slot.and_then(|(role, index)| hovered_stack(game, role, *index as usize))
    };
    populate_stack(stack, state)
}

pub(super) fn populate_stack(
    stack: Option<ItemStack>,
    state: &mut UiState,
) -> Vec<(String, std::path::PathBuf)> {
    let stack =
        stack.filter(|stack| stack.item != petramond_world::item::ItemType::Air && stack.count > 0);
    state.set("show_item_tip", UiValue::Bool(stack.is_some()));
    state.set(
        "item_tip_name",
        UiValue::Str(
            stack
                .map(|stack| stack.item.name().to_owned())
                .unwrap_or_default(),
        ),
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
    let data = stack.and_then(|s| petramond_world::item::variant::get(s.variant));
    let info = data
        .as_ref()
        .and_then(|d| d.get(petramond_world::item::variant::INFO_DATA_KEY))
        .and_then(|v| std::str::from_utf8(v).ok())
        .unwrap_or_default();
    let (text, icons) = info.split_once('\n').unwrap_or((info, ""));
    state.set("item_tip_instance", UiValue::Str(text.to_owned()));
    let icons: Vec<_> = icons
        .split(',')
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
        .take(4)
        .collect();
    let mut images = Vec::new();
    for i in 0..4 {
        let name = icons.get(i).copied().unwrap_or_default();
        let path = (!name.is_empty())
            .then(|| {
                petramond_world::assets::candidate_paths(&format!("ui/icons/{name}.png"))
                    .into_iter()
                    .find(|p| p.is_file())
            })
            .flatten();
        let key = format!("item_tip_icon{i}");
        state.set(
            &key,
            UiValue::Str(if path.is_some() {
                name.to_owned()
            } else {
                String::new()
            }),
        );
        if let Some(path) = path {
            images.push((name.to_owned(), path));
        }
    }
    images
}

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
                    .filter(|s| s.item != petramond_world::item::ItemType::Air && s.count > 0)
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

fn hovered_stack(game: &Game, role: &str, index: usize) -> Option<ItemStack> {
    let menu = game.menu_read_model();
    match Role::from_key(role)? {
        Role::Hotbar => menu.inventory.slot(index).copied(),
        Role::PlayerInv => menu.inventory.slot(HOTBAR_LEN + index).copied(),
        Role::OffHand => menu.inventory.off_hand().copied(),
        Role::CraftResult => menu.craft_output,
        Role::Container => menu
            .container
            .and_then(|container| container.slots.get(index).copied().flatten()),
        Role::Generic | Role::Other => None,
    }
}
