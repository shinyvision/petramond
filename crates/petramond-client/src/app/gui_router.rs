use petramond_world::gui_state::MenuSlot;
use petramond_world::gui_state::PointerButton;

#[derive(Default, Debug)]
pub(super) struct GuiRouter {
    double_click: DoubleClickStreak,
}

impl GuiRouter {
    pub(super) fn reset_click_streak(&mut self) {
        self.double_click.reset();
    }

    pub(super) fn doc_gather(
        &mut self,
        slot: MenuSlot,
        button: PointerButton,
        shift: bool,
        now: f64,
        cursor_has_stack: bool,
    ) -> bool {
        self.left_click_gather(slot, button, shift, now, cursor_has_stack)
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
            MenuSlot::Container(i) => Some(CONTAINER_SLOT_STREAK_BASE + i),
            MenuSlot::OffHand => Some(OFF_HAND_STREAK_KEY),
            MenuSlot::CraftResult | MenuSlot::Widget(_) => None,
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

const DOUBLE_CLICK_SECS: f64 = 0.25;

const CONTAINER_SLOT_STREAK_BASE: usize = 1000;

const OFF_HAND_STREAK_KEY: usize = 999;
