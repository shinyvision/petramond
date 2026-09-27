use super::Game;
use petramond::net::protocol::{ClientToServer, PlayerAction};
use petramond::schematic::{CreativeAction, CreativeReply, Schematic};
use std::sync::Arc;

const DOUBLE_TAP_SECONDS: f64 = 0.3;
const BREAK_REPEAT_SECONDS: f32 = 0.2;

#[derive(Default)]
pub struct FlightToggle {
    last_jump: Option<f64>,
}

impl FlightToggle {
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
        self.local.player.is_flying()
    }

    pub fn creative_mode(&self) -> bool {
        self.local.player.is_creative()
    }

    pub fn toggle_creative_mode(&mut self) {
        self.net
            .queue(ClientToServer::Action(PlayerAction::ToggleCreative));
    }

    pub fn jump_pressed(&mut self, now: f64) {
        if self.local.player.is_creative() && self.local.flight_toggle.press(now) {
            self.net
                .queue(ClientToServer::Action(PlayerAction::ToggleFlight));
        }
    }

    pub fn undo_edit(&mut self) {
        if let Some(tool) = self.editing_tool() {
            tool.undo();
        } else if self.local.player.is_creative() {
            self.send_creative(CreativeAction::Undo);
        }
    }

    pub fn redo_edit(&mut self) {
        if let Some(tool) = self.editing_tool() {
            tool.redo();
        } else if self.local.player.is_creative() {
            self.send_creative(CreativeAction::Redo);
        }
    }

    fn send_creative(&mut self, action: CreativeAction) {
        self.net
            .queue(ClientToServer::Action(PlayerAction::Creative(action)));
    }

    pub fn save_selection(&mut self, name: &str, include_air: bool) {
        let selection = &self.tools.world.selection.selection;
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
        self.tools.world.selection.clear();
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
        self.tools.library.queue_save(schematic);
        self.notice.clear();
    }
}
