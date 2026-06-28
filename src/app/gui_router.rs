use super::App;
use crate::controls::PointerButton;
use crate::gui::{GuiKind, MenuSlot};

/// App-side GUI click router: converts the current menu hit-test into the small
/// set of game actions App is allowed to apply.
#[derive(Default, Debug)]
pub(super) struct GuiRouter {
    double_click: DoubleClickStreak,
}

#[derive(Copy, Clone, Debug)]
struct GuiClick {
    open: bool,
    kind: GuiKind,
    screen: (u32, u32),
    cursor: (f32, f32),
    button: PointerButton,
    shift: bool,
    now: f64,
    cursor_has_stack: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct RoutedGuiClick {
    consumed: bool,
    action: Option<GuiClickAction>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum GuiClickAction {
    MenuClick {
        slot: MenuSlot,
        button: PointerButton,
        shift: bool,
        gather: bool,
    },
    ThrowCursorStack,
    ThrowCursorOne,
}

impl GuiRouter {
    fn route_click(&mut self, click: GuiClick) -> RoutedGuiClick {
        if !click.open {
            return RoutedGuiClick {
                consumed: false,
                action: None,
            };
        }

        let action = match crate::gui::hit(click.kind, click.screen, click.cursor) {
            Some(slot) => {
                let gather = self.left_click_gather(
                    slot,
                    click.button,
                    click.shift,
                    click.now,
                    click.cursor_has_stack,
                );
                Some(GuiClickAction::MenuClick {
                    slot,
                    button: click.button,
                    shift: click.shift,
                    gather,
                })
            }
            None if !crate::gui::panel_contains(click.kind, click.screen, click.cursor) => {
                self.reset_click_streak();
                Some(match click.button {
                    PointerButton::Primary => GuiClickAction::ThrowCursorStack,
                    PointerButton::Secondary => GuiClickAction::ThrowCursorOne,
                })
            }
            None => None,
        };

        RoutedGuiClick {
            consumed: true,
            action,
        }
    }

    pub(super) fn reset_click_streak(&mut self) {
        self.double_click.reset();
    }

    fn left_click_gather(
        &mut self,
        slot: MenuSlot,
        button: PointerButton,
        shift: bool,
        now: f64,
        cursor_has_stack: bool,
    ) -> bool {
        let streak_key = match slot {
            _ if shift || button != PointerButton::Primary => None,
            MenuSlot::Inventory(i) => Some(i),
            MenuSlot::Chest(i) => Some(CHEST_SLOT_STREAK_BASE + i),
            MenuSlot::Craft(_) | MenuSlot::Furnace(_) | MenuSlot::Workbench(_) => None,
        };
        match streak_key {
            Some(key) => self.double_click.register(key, now) && cursor_has_stack,
            None => {
                self.reset_click_streak();
                false
            }
        }
    }
}

impl App {
    /// Route a left-click to the open inventory. Returns whether it was consumed
    /// (i.e. the inventory was open). No-op when closed. `now` timestamps the click
    /// for double-click detection.
    pub(super) fn route_screen_click(&mut self, screen: (u32, u32), now: f64) -> bool {
        self.route_gui_click(screen, PointerButton::Primary, now)
    }

    /// Route a right-click to the open inventory. Returns whether it was consumed
    /// (i.e. the inventory was open) — so a closed-inventory right-click falls
    /// through to block placement. No-op when closed.
    pub(super) fn route_screen_right_click(&mut self, screen: (u32, u32), now: f64) -> bool {
        self.route_gui_click(screen, PointerButton::Secondary, now)
    }

    fn route_gui_click(&mut self, screen: (u32, u32), button: PointerButton, now: f64) -> bool {
        let routed = self.gui_router.route_click(GuiClick {
            open: self.screen.ui_open(),
            kind: self.screen.gui_kind(),
            screen,
            cursor: self.pointer.cursor(),
            button,
            shift: self.modifiers.shift,
            now,
            cursor_has_stack: self.game.cursor_has_stack(),
        });
        if let Some(action) = routed.action {
            self.apply_gui_click_action(action);
        }
        routed.consumed
    }

    fn apply_gui_click_action(&mut self, action: GuiClickAction) {
        match action {
            GuiClickAction::MenuClick {
                slot,
                button,
                shift,
                gather,
            } => {
                self.game.menu_click(slot, button, shift, gather);
            }
            GuiClickAction::ThrowCursorStack => self.game.throw_cursor_stack(),
            GuiClickAction::ThrowCursorOne => self.game.throw_cursor_one(),
        }
    }
}

#[derive(Default, Debug)]
struct DoubleClickStreak {
    slot: Option<usize>,
    time: f64,
}

impl DoubleClickStreak {
    fn register(&mut self, slot: usize, now: f64) -> bool {
        let is_double = self.slot == Some(slot) && now - self.time < DOUBLE_CLICK_SECS;
        if is_double {
            self.slot = None;
        } else {
            self.slot = Some(slot);
            self.time = now;
        }
        is_double
    }

    fn reset(&mut self) {
        self.slot = None;
    }
}

/// Matches the classic ~250 ms double-click timeout for gather-on-cursor.
const DOUBLE_CLICK_SECS: f64 = 0.25;

/// Namespaces chest storage slots away from inventory slots in the click streak.
const CHEST_SLOT_STREAK_BASE: usize = 1000;
