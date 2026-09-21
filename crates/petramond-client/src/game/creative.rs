//! What only creative mode has: the mode and flight toggles, instant-break
//! pacing, the server's edit history, and capturing a selection for saving.

use super::Game;
use petramond::net::protocol::{ClientToServer, PlayerAction};
use petramond::schematic::{CreativeAction, CreativeReply, Schematic};
use std::sync::Arc;

/// Two jump presses this close together toggle flight.
const DOUBLE_TAP_SECONDS: f64 = 0.3;
/// Pause between instant breaks while the button stays held.
const BREAK_REPEAT_SECONDS: f32 = 0.2;

#[derive(Default)]
pub struct FlightToggle {
    last_jump: Option<f64>,
}

impl FlightToggle {
    /// Whether this press completes a double tap.
    fn press(&mut self, now: f64) -> bool {
        let double = self
            .last_jump
            .take()
            .is_some_and(|last| now - last < DOUBLE_TAP_SECONDS);
        if !double {
            self.last_jump = Some(now);
        }
        double
    }
}

/// Spaces out held instant breaks, so one press does not tear through a wall.
#[derive(Default)]
pub struct BreakRepeat {
    wait: f32,
}

impl BreakRepeat {
    pub(super) fn tick(&mut self, dt: f32) {
        self.wait = (self.wait - dt.max(0.0)).max(0.0);
    }

    pub(super) fn ready(&self) -> bool {
        self.wait == 0.0
    }

    pub(super) fn arm(&mut self) {
        self.wait = BREAK_REPEAT_SECONDS;
    }
}

impl Game {
    pub fn creative_flying(&self) -> bool {
        self.player.is_flying()
    }

    pub fn creative_mode(&self) -> bool {
        self.player.is_creative()
    }

    pub fn toggle_creative_mode(&mut self) {
        self.outbox
            .push(ClientToServer::Action(PlayerAction::ToggleCreative));
    }

    pub fn jump_pressed(&mut self, now: f64) {
        if self.player.is_creative() && self.flight_toggle.press(now) {
            self.outbox
                .push(ClientToServer::Action(PlayerAction::ToggleFlight));
        }
    }

    /// Undo the held tool's last edit; without one (or under a placement
    /// preview) the server's last creative edit.
    pub fn undo_edit(&mut self) {
        if let Some(tool) = self.editing_tool() {
            tool.undo();
        } else if self.player.is_creative() {
            self.send_creative(CreativeAction::Undo);
        }
    }

    pub fn redo_edit(&mut self) {
        if let Some(tool) = self.editing_tool() {
            tool.redo();
        } else if self.player.is_creative() {
            self.send_creative(CreativeAction::Redo);
        }
    }

    fn send_creative(&mut self, action: CreativeAction) {
        self.outbox
            .push(ClientToServer::Action(PlayerAction::Creative(action)));
    }

    /// Ask the server to capture the selection as `name`; the cells come
    /// back as a blob and are saved to the library.
    pub fn save_selection(&mut self, name: &str, include_air: bool) {
        let selection = &self.world_tools.selection.selection;
        if selection.is_empty() {
            self.notice = "Select blocks first".into();
            return;
        }
        if name.trim().is_empty() {
            self.notice = "Enter a name for the schematic".into();
            return;
        }
        let regions = selection.regions().to_vec();
        self.send_creative(CreativeAction::Capture {
            name: name.trim().into(),
            regions,
            include_air,
        });
        self.notice.clear();
    }

    pub fn clear_selection(&mut self) {
        self.world_tools.selection.clear();
        self.notice.clear();
    }

    pub(super) fn receive_creative_replies(&mut self, replies: Vec<CreativeReply>) {
        for reply in replies {
            match reply {
                CreativeReply::Message(message) => self.notice = message,
                CreativeReply::Captured { digest } => self.expect_capture(digest),
            }
        }
    }

    pub(crate) fn schematic_captured(&mut self, schematic: Arc<Schematic>) {
        self.schematic_library.queue_save(schematic);
        self.notice.clear();
    }
}
