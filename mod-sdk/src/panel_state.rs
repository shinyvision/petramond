use std::collections::BTreeMap;

use crate::{gui_state_set_for, FxHashMap, GuiValue, GuiViewerData};

pub trait PanelState {
    fn values(&self) -> Vec<(&'static str, GuiValue)>;
}

pub fn gui_flag(on: bool) -> GuiValue {
    GuiValue::I32(i32::from(on))
}

pub fn gui_text(text: impl Into<String>) -> GuiValue {
    GuiValue::Str(text.into())
}

#[derive(Default)]
pub struct PanelPublisher {
    sent: FxHashMap<Viewer, BTreeMap<&'static str, GuiValue>>,
}

type Viewer = (crate::PlayerId, String, Option<crate::ContainerAddress>);

fn key_of(viewer: &GuiViewerData) -> Viewer {
    (viewer.player_id, viewer.kind.clone(), viewer.anchor)
}

impl PanelPublisher {
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

    pub fn is_fresh(&self, viewer: &GuiViewerData) -> bool {
        self.sent
            .get(&key_of(viewer))
            .is_none_or(|sent| sent.is_empty())
    }

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

    pub fn opened(&mut self, at: Option<crate::ContainerAddress>) {
        for (key, sent) in &mut self.sent {
            if key.2 == at {
                sent.clear();
            }
        }
    }

    pub fn resend(&mut self, viewer: &GuiViewerData) {
        if let Some(sent) = self.sent.get_mut(&key_of(viewer)) {
            sent.clear();
        }
    }
}
