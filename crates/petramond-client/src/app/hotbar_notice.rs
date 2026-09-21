//! The small transient line above the hotbar: a held world tool announcing
//! its setting, and refusals or failures the game reports. Held two seconds,
//! then faded over one.

use petramond_ui::{UiState, UiValue};

#[derive(Default)]
pub(in crate::app) struct HotbarNotice {
    /// The hotbar slot and setting label last announced.
    previous: Option<(u8, &'static str)>,
    started: f64,
    text: String,
    is_setting: bool,
}

impl HotbarNotice {
    pub(in crate::app) fn populate(
        &mut self,
        held: Option<(u8, &'static str)>,
        pending: &mut String,
        now: f64,
        state: &mut UiState,
    ) {
        if held != self.previous {
            self.previous = held;
            if self.is_setting || now - self.started >= 3.0 || self.text.is_empty() {
                self.text = held.map_or("", |(_, label)| label).into();
                self.is_setting = true;
                self.started = now;
            }
        }
        if !pending.is_empty() {
            self.text = std::mem::take(pending);
            self.is_setting = false;
            self.started = now;
        }
        let age = (now - self.started).max(0.0);
        let opacity = if !self.text.is_empty() {
            (3.0 - age).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        state.set("creative_notice_visible", UiValue::Bool(opacity > 0.0));
        state.set("creative_notice_opacity", UiValue::F32(opacity));
        state.set("creative_notice", UiValue::Str(self.text.clone()));
    }
}

#[cfg(test)]
mod tests;
