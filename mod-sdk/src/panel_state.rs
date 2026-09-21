//! Publishing a mod GUI's state map without resending what has not changed.
//!
//! A panel's labels, flags and gauges are keys of its session's state map
//! ([`gui_state_set_for`]). Describe a panel as a plain struct, list its keys
//! in ONE place ([`PanelState::values`]), and let a [`PanelPublisher`] send
//! only the keys whose value changed since it last sent them to that viewer.
//!
//! A session's state map starts EMPTY each time a GUI opens, and an unset
//! `enabled`/`visible` bind reads as true. So publish from a tick system
//! attached AFTER the menu stage, and fill a panel seen for the first time
//! at once ([`PanelPublisher::is_fresh`]) whatever cadence the rest keep.

use std::collections::BTreeMap;

use crate::{gui_state_set_for, FxHashMap, GuiValue, GuiViewerData};

/// A panel's whole state, as the keys its document binds.
pub trait PanelState {
    fn values(&self) -> Vec<(&'static str, GuiValue)>;
}

/// `GuiValue` for an `enabled` / `visible` / frame-index bind.
pub fn gui_flag(on: bool) -> GuiValue {
    GuiValue::I32(i32::from(on))
}

pub fn gui_text(text: impl Into<String>) -> GuiValue {
    GuiValue::Str(text.into())
}

/// What each open panel was last sent, by viewer (a session has one GUI open:
/// who, which kind, anchored where).
#[derive(Default)]
pub struct PanelPublisher {
    sent: FxHashMap<Viewer, BTreeMap<&'static str, GuiValue>>,
}

type Viewer = (crate::PlayerId, String, Option<crate::ContainerAddress>);

fn key_of(viewer: &GuiViewerData) -> Viewer {
    (viewer.player_id, viewer.kind.clone(), viewer.anchor)
}

impl PanelPublisher {
    /// Take in this tick's [`gui_viewers`](crate::gui_viewers): panels no
    /// longer open are forgotten (their state map is gone with them) and
    /// returned, for whatever a mod does when its panel closes.
    pub fn observe(&mut self, open: &[GuiViewerData]) -> Vec<GuiViewerData> {
        let mut closed = Vec::new();
        self.sent.retain(|key, _| {
            let still = open.iter().any(|viewer| key_of(viewer) == *key);
            if !still {
                closed.push(GuiViewerData {
                    player_id: key.0,
                    kind: key.1.clone(),
                    anchor: key.2,
                });
            }
            still
        });
        closed
    }

    /// Nothing this panel still holds has been sent to it: fill it now.
    pub fn is_fresh(&self, viewer: &GuiViewerData) -> bool {
        self.sent
            .get(&key_of(viewer))
            .is_none_or(|sent| sent.is_empty())
    }

    /// Send the keys of `state` that differ from what this viewer has.
    pub fn publish(&mut self, viewer: &GuiViewerData, state: &impl PanelState) {
        let sent = self.sent.entry(key_of(viewer)).or_default();
        for (key, value) in state.values() {
            if sent.get(key) == Some(&value) {
                continue;
            }
            if gui_state_set_for(viewer.player_id, key, value.clone()) {
                sent.insert(key, value);
            }
        }
    }

    /// A GUI session began on `at` (a `container_opened` event): whoever has
    /// a panel there may be looking at a new, empty state map, so they are
    /// fresh again. Catches a panel closed and opened again between two
    /// ticks, which [`PanelPublisher::observe`] cannot tell from one that
    /// stayed open.
    pub fn opened(&mut self, at: Option<crate::ContainerAddress>) {
        for (key, sent) in &mut self.sent {
            if key.2 == at {
                sent.clear();
            }
        }
    }

    /// Forget what this viewer was sent: it is fresh again, and the next
    /// publish sends every key. Cheap insurance on a slow cadence against a state map that was reset
    /// without the panel ever reading as closed.
    pub fn resend(&mut self, viewer: &GuiViewerData) {
        if let Some(sent) = self.sent.get_mut(&key_of(viewer)) {
            sent.clear();
        }
    }
}
