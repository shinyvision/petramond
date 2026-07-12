use crate::controls::PointerButton;
use crate::gui::MenuSlot;

/// App-side GUI click state: double-click gather detection for the
/// document-routed slot clicks (`drive_doc_menu`). The document runtime owns
/// hit testing; this only decides whether a click is a gather.
#[derive(Default, Debug)]
pub(super) struct GuiRouter {
    double_click: DoubleClickStreak,
}

impl GuiRouter {
    pub(super) fn reset_click_streak(&mut self) {
        self.double_click.reset();
    }

    /// Double-click gather detection for document-routed slot clicks (the
    /// same streak the legacy hit-test path uses).
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
            MenuSlot::Chest(i) => Some(CHEST_SLOT_STREAK_BASE + i),
            // Mod container slots skip the gather streak (like the furnace's
            // semantic slots): the double-click sweep is a plain-storage read.
            MenuSlot::CraftResult
            | MenuSlot::Furnace(_)
            | MenuSlot::Workbench(_)
            | MenuSlot::Container(_)
            | MenuSlot::Widget(_) => None,
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

/// Matches the classic ~250 ms double-click timeout for gather-on-cursor.
const DOUBLE_CLICK_SECS: f64 = 0.25;

/// Namespaces chest storage slots away from inventory slots in the click streak.
const CHEST_SLOT_STREAK_BASE: usize = 1000;
