use std::collections::BTreeMap;

use mod_api::{ClientPointerButton, ClientPointerPhase, ClientUiEvent};
use petramond_ui::{FrameOutput, PointerButton, PointerPhase, UiEvent};
use petramond_world::gui_state::GuiKind;

pub(super) fn client_ui_event(event: UiEvent) -> Option<ClientUiEvent> {
    if super::shell_docs::is_secondary_activation(&event) {
        return None;
    }
    Some(match event {
        UiEvent::Click { id, item, .. } => ClientUiEvent::Click { id, item },
        UiEvent::Toggle { id, item, on, .. } => ClientUiEvent::Toggle { id, item, on },
        UiEvent::SliderChange {
            id,
            item,
            value,
            committed,
        } => ClientUiEvent::Slider {
            id,
            item,
            value,
            committed,
        },
        UiEvent::TextChanged { id, text } => ClientUiEvent::TextChanged { id, text },
        UiEvent::Submit { id, text } => ClientUiEvent::Submit { id, text },
        UiEvent::Blur { id, item } => ClientUiEvent::Blur { id, item },
        UiEvent::ListSelect { id, index } => ClientUiEvent::ListSelect { id, index },
        UiEvent::ListActivate { id, index } => ClientUiEvent::ListActivate { id, index },
        UiEvent::TabSelect { id, index } => ClientUiEvent::TabSelect { id, index },
        UiEvent::SurfacePointer {
            id,
            item,
            phase,
            x,
            y,
            button,
            mods,
            clicks,
        } => ClientUiEvent::CanvasPointer {
            id,
            item,
            phase: pointer_phase(phase),
            x,
            y,
            button: button.map(pointer_button),
            mods: key_mods(mods),
            clicks,
        },
        UiEvent::SurfaceScroll {
            id,
            item,
            x,
            y,
            delta,
            mods,
        } => ClientUiEvent::CanvasScroll {
            id,
            item,
            x,
            y,
            delta: -(delta as f32) / super::pointer::WHEEL_NOTCH_PX,
            mods: key_mods(mods),
        },
        UiEvent::SurfaceSize { id, item, w, h } => ClientUiEvent::CanvasSize {
            id,
            item,
            w: w.max(0) as u32,
            h: h.max(0) as u32,
        },
        UiEvent::ImagePointer {
            id,
            phase,
            x,
            y,
            button,
        } => ClientUiEvent::ImagePointer {
            id,
            phase: pointer_phase(phase),
            x,
            y,
            button: pointer_button(button),
        },
        _ => return None,
    })
}

pub(super) fn pointer_phase(phase: PointerPhase) -> ClientPointerPhase {
    match phase {
        PointerPhase::Down => ClientPointerPhase::Down,
        PointerPhase::Move => ClientPointerPhase::Move,
        PointerPhase::Up => ClientPointerPhase::Up,
        PointerPhase::Leave => ClientPointerPhase::Leave,
    }
}

fn key_mods(mods: petramond_ui::Mods) -> mod_api::ClientKeyMods {
    mod_api::ClientKeyMods {
        ctrl: mods.ctrl,
        shift: mods.shift,
        alt: mods.alt,
    }
}

pub(super) fn pointer_button(button: PointerButton) -> ClientPointerButton {
    match button {
        PointerButton::Primary => ClientPointerButton::Primary,
        PointerButton::Secondary => ClientPointerButton::Secondary,
    }
}

#[derive(Default)]
pub(crate) struct DocWatch {
    kind: Option<GuiKind>,
    hover: Option<(Option<String>, Option<u32>)>,
    ranges: BTreeMap<String, (u32, u32)>,
}

impl DocWatch {
    pub(crate) fn changes(&mut self, kind: GuiKind, out: &FrameOutput) -> Vec<ClientUiEvent> {
        if self.kind != Some(kind) {
            *self = DocWatch {
                kind: Some(kind),
                ..DocWatch::default()
            };
        }
        let mut events = Vec::new();
        let hover = out
            .hover_widget
            .as_ref()
            .map_or((None, None), |k| (Some(k.id.clone()), k.item));
        if self.hover.as_ref() != Some(&hover) {
            events.push(ClientUiEvent::Hover {
                id: hover.0.clone(),
                item: hover.1,
            });
            self.hover = Some(hover);
        }
        for (key, first, count) in &out.list_ranges {
            if key.item.is_some() {
                continue;
            }
            if self.ranges.get(&key.id) != Some(&(*first, *count)) {
                self.ranges.insert(key.id.clone(), (*first, *count));
                events.push(ClientUiEvent::ListRange {
                    id: key.id.clone(),
                    first: *first,
                    count: *count,
                });
            }
        }
        events
    }

    pub(super) fn forget(&mut self) {
        *self = DocWatch::default();
    }
}
