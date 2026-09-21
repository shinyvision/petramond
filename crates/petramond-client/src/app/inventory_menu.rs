use super::App;
use petramond_world::gui_state::GuiKind;

impl App {
    pub(in crate::app) fn handle_inventory_event(
        &mut self,
        kind: GuiKind,
        ev: petramond_ui::UiEvent,
        now: f64,
    ) {
        let to_button = |b| match b {
            petramond_ui::PointerButton::Primary => {
                petramond_world::gui_state::PointerButton::Primary
            }
            petramond_ui::PointerButton::Secondary => {
                petramond_world::gui_state::PointerButton::Secondary
            }
        };
        match ev {
            petramond_ui::UiEvent::SlotClick {
                role,
                index,
                button,
                shift,
            } => {
                let Some(slot) =
                    petramond::gui::Role::from_key(&role).and_then(|r| r.menu_slot(index as usize))
                else {
                    return;
                };
                let button = to_button(button);
                let cursor_has_stack = self.game.as_ref().is_some_and(|g| g.cursor_has_stack());
                let gather = self
                    .gui_router
                    .doc_gather(slot, button, shift, now, cursor_has_stack);
                if let Some(game) = self.game.as_mut() {
                    game.menu_click(slot, button, shift, gather);
                }
            }
            petramond_ui::UiEvent::SlotDrag { slots, button } => {
                self.gui_router.reset_click_streak();
                let slots = slots
                    .into_iter()
                    .filter_map(|(role, index)| {
                        petramond::gui::Role::from_key(&role)
                            .and_then(|role| role.menu_slot(index as usize))
                    })
                    .collect();
                if let Some(game) = self.game.as_mut() {
                    game.menu_drag(kind, slots, to_button(button));
                }
            }
            petramond_ui::UiEvent::ClickOutside { button } => {
                self.gui_router.reset_click_streak();
                if let Some(game) = self.game.as_mut() {
                    use petramond::net::protocol::ThrowAmount;
                    game.throw_cursor(match to_button(button) {
                        petramond_world::gui_state::PointerButton::Primary => ThrowAmount::All,
                        petramond_world::gui_state::PointerButton::Secondary => ThrowAmount::One,
                    });
                }
            }
            _ => {}
        }
    }
}
