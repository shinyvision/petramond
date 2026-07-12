//! The app's GUI-document driver: owns the per-open-screen ephemeral widget
//! state ([`FrameState`]), queues host input as [`InputEvent`]s, runs one
//! [`UiRuntime`] frame per draw, and hands the resulting events to whoever
//! owns the screen (shell controllers app-side; slot clicks latch to the
//! tick).
//!
//! Nothing here touches `Game`/`World` — ephemeral widget state can never
//! leak into the deterministic tick. The only tick-bound artifact is a
//! [`UiEvent`] the caller explicitly latches.

use crate::gui::{doc_theme, documents, GuiKind};
use petramond_ui::{DocImages, FrameArgs, FrameOutput, FrameState, InputEvent, UiRuntime, UiState};

/// A document's image registry: document-local images first, then the
/// host-registered extras (controller-provided icons), in one
/// `TexId::DocImage` index space (the renderer uploads the same order).
struct DocImageSet<'a> {
    doc: std::sync::Arc<Vec<documents::DocImageRef>>,
    extra: &'a [documents::DocImageRef],
    dynamic: &'a [crate::modding::ClientImageData],
}

impl DocImages for DocImageSet<'_> {
    fn resolve(&self, name: &str) -> Option<(u16, (u32, u32))> {
        if let Some(idx) = self.doc.iter().position(|i| i.name == name) {
            return Some((idx as u16, self.doc[idx].size));
        }
        if let Some(idx) = self.extra.iter().position(|i| i.name == name) {
            return Some(((self.doc.len() + idx) as u16, self.extra[idx].size));
        }
        self.dynamic.iter().position(|i| i.key == name).map(|idx| {
            (
                (self.doc.len() + self.extra.len() + idx) as u16,
                (
                    self.dynamic[idx].width as u32,
                    self.dynamic[idx].height as u32,
                ),
            )
        })
    }
}

pub(super) struct AppUi {
    fs: FrameState,
    out: FrameOutput,
    state: UiState,
    input: Vec<InputEvent>,
    active: Option<GuiKind>,
    /// Host-registered images beyond the document's own (per-row icons the
    /// controller names via `bind.image`), appended to the `DocImage` space.
    extra_images: Vec<documents::DocImageRef>,
    dynamic_images: Vec<crate::modding::ClientImageData>,
    /// This frame's `DocImage` index → source order (renderer upload).
    image_sources: Vec<crate::gui::DocImageSource>,
    viewport_generation: u64,
    frame_stamp: Option<(GuiKind, crate::gui::UiViewport)>,
    /// Native clipboard by default; tests inject an in-memory one so text
    /// tests never touch the OS.
    clipboard: Box<dyn petramond_ui::TextClipboard>,
}

/// Lazy native clipboard for document text inputs (its own arboard handle —
/// independent of the platform shell clipboard, which dies with the legacy
/// path).
#[derive(Default)]
struct DocClipboard {
    inner: Option<arboard::Clipboard>,
    tried: bool,
}

impl DocClipboard {
    fn ensure(&mut self) -> Option<&mut arboard::Clipboard> {
        if !self.tried {
            self.tried = true;
            self.inner = arboard::Clipboard::new().ok();
        }
        self.inner.as_mut()
    }
}

impl petramond_ui::TextClipboard for DocClipboard {
    fn get_text(&mut self) -> Option<String> {
        self.ensure()?.get_text().ok()
    }
    fn set_text(&mut self, text: &str) -> bool {
        self.ensure()
            .map(|c| c.set_text(text.to_owned()).is_ok())
            .unwrap_or(false)
    }
}

impl AppUi {
    pub fn new() -> AppUi {
        AppUi {
            fs: FrameState::new(),
            out: FrameOutput::default(),
            state: UiState::new(),
            input: Vec::new(),
            active: None,
            extra_images: Vec::new(),
            dynamic_images: Vec::new(),
            image_sources: Vec::new(),
            viewport_generation: 0,
            frame_stamp: None,
            clipboard: Box::new(DocClipboard::default()),
        }
    }

    #[cfg(test)]
    pub fn set_clipboard(&mut self, clipboard: Box<dyn petramond_ui::TextClipboard>) {
        self.clipboard = clipboard;
    }

    pub fn clipboard_mut(&mut self) -> &mut dyn petramond_ui::TextClipboard {
        self.clipboard.as_mut()
    }

    /// Whether `kind` is backed by a loaded GUI document.
    pub fn doc_backed(kind: GuiKind) -> bool {
        documents::doc_for(kind).is_some()
    }

    /// Queue a host input event for the next frame.
    pub fn push_input(&mut self, ev: InputEvent) {
        self.input.push(ev);
    }

    /// The state map the active screen's controller populates.
    pub fn state_mut(&mut self) -> &mut UiState {
        &mut self.state
    }

    /// Register controller-provided images (per-row icons) for the active
    /// screen; names resolve via `bind.image`. Sizes read once per path.
    pub fn set_extra_images(&mut self, images: &[(String, std::path::PathBuf)]) {
        if self.extra_images.len() == images.len()
            && self
                .extra_images
                .iter()
                .zip(images)
                .all(|(a, (name, path))| &a.name == name && &a.path == path)
        {
            return;
        }
        self.extra_images = images
            .iter()
            .filter_map(|(name, path)| {
                let size = image::image_dimensions(path).ok()?;
                Some(documents::DocImageRef {
                    name: name.clone(),
                    path: path.clone(),
                    size,
                })
            })
            .collect();
    }

    pub fn set_dynamic_images(&mut self, images: Vec<crate::modding::ClientImageData>) {
        self.dynamic_images = images;
    }

    pub fn replace_client_state(
        &mut self,
        state: &std::collections::BTreeMap<String, mod_api::GuiValue>,
    ) {
        self.state.clear();
        for (key, value) in state {
            let value = match value {
                mod_api::GuiValue::F32(v) => petramond_ui::UiValue::F32(*v),
                mod_api::GuiValue::I32(v) => petramond_ui::UiValue::I32(*v),
                mod_api::GuiValue::Str(v) => petramond_ui::UiValue::Str(v.clone()),
            };
            self.state.set(key.clone(), value);
        }
    }

    pub fn image_sources(&self) -> &[crate::gui::DocImageSource] {
        &self.image_sources
    }

    pub fn text_input_focused(&self) -> bool {
        self.fs.focused().is_some()
    }

    pub fn set_viewport_generation(&mut self, generation: u64) {
        self.viewport_generation = generation;
    }

    pub fn frame_stamp(&self) -> Option<(GuiKind, crate::gui::UiViewport)> {
        self.frame_stamp
    }

    /// Programmatically focus a text input (pre-loaded with `text`), as if
    /// clicked — controllers use this when they reveal an inline editor.
    pub fn focus_text_input(&mut self, id: &str, text: &str, max_chars: usize) {
        self.fs.focus_text_input(
            petramond_ui::InstKey {
                id: id.to_owned(),
                item: None,
            },
            text,
            max_chars,
        );
    }

    /// Reset ephemeral widget state + bound state when the screen changes,
    /// BEFORE the new screen's controller populates.
    pub fn ensure_active(&mut self, kind: GuiKind) {
        if self.active != Some(kind) {
            self.fs.reset();
            self.state.clear();
            self.extra_images.clear();
            self.dynamic_images.clear();
            self.frame_stamp = None;
            self.active = Some(kind);
        }
    }

    /// Run one runtime frame for `kind`; queued input drains into this frame.
    /// `dim` is the full-screen backdrop quad painted behind the tree
    /// (`None` = none) — screens over live gameplay pass their own colour
    /// (menu dim, sleep fade, death tint). Returns `false` (and draws
    /// nothing) when no document backs `kind`.
    pub fn frame(
        &mut self,
        kind: GuiKind,
        screen: (u32, u32),
        now: f64,
        dim: Option<[f32; 4]>,
    ) -> bool {
        let Some(doc) = documents::doc_for(kind) else {
            self.input.clear();
            self.frame_stamp = None;
            return false;
        };
        self.ensure_active(kind);
        let viewport = crate::gui::UiViewport::new(screen, self.viewport_generation);
        let rt = UiRuntime::new(doc.doc, doc_theme::theme());
        self.image_sources.clear();
        self.image_sources.extend(
            doc.images
                .iter()
                .map(|i| crate::gui::DocImageSource::Path(i.path.clone())),
        );
        self.image_sources.extend(
            self.extra_images
                .iter()
                .map(|i| crate::gui::DocImageSource::Path(i.path.clone())),
        );
        self.image_sources
            .extend(
                self.dynamic_images
                    .iter()
                    .map(|i| crate::gui::DocImageSource::Dynamic {
                        key: i.key.clone(),
                        size: (i.width as u32, i.height as u32),
                        revision: i.revision,
                        rgba: i.rgba.clone(),
                    }),
            );
        let images = DocImageSet {
            doc: doc.images,
            extra: &self.extra_images,
            dynamic: &self.dynamic_images,
        };
        let input = std::mem::take(&mut self.input);
        rt.frame(
            FrameArgs {
                screen: viewport.size,
                scale: viewport.scale,
                now,
                state: &self.state,
                input: &input,
                clipboard: Some(self.clipboard.as_mut()),
                images: &images,
                dim,
                preview: None,
            },
            &mut self.fs,
            &mut self.out,
        );
        self.frame_stamp = Some((kind, viewport));
        true
    }

    /// The events the last frame resolved (drained).
    pub fn take_events(&mut self) -> Vec<petramond_ui::UiEvent> {
        std::mem::take(&mut self.out.events)
    }

    /// The last frame's output (draw list + rects).
    pub fn out(&self) -> &FrameOutput {
        &self.out
    }

    pub fn draw_mut(&mut self) -> &mut petramond_ui::DrawList {
        &mut self.out.draw
    }

    /// The last frame's slot cells as game-typed [`crate::gui::DocSlot`]s
    /// (unknown roles drop — they can't own game content).
    pub fn doc_slots(&self) -> std::sync::Arc<Vec<crate::gui::DocSlot>> {
        std::sync::Arc::new(
            self.out
                .slots
                .iter()
                .filter_map(|s| {
                    let role = crate::gui::Role::from_key(&s.role)?;
                    Some(crate::gui::DocSlot::new(
                        role,
                        s.index,
                        crate::gui::SlotRect {
                            x: s.rect.x as f32,
                            y: s.rect.y as f32,
                            w: s.rect.w as f32,
                            h: s.rect.h as f32,
                        },
                    ))
                })
                .collect(),
        )
    }

    /// Recipe-browser host hooks from the same solved frame as `doc_slots`.
    /// Unknown hook ids and non-list instances are deliberately ignored.
    pub fn doc_hooks(&self) -> std::sync::Arc<Vec<crate::gui::DocHook>> {
        std::sync::Arc::new(
            self.out
                .hooks
                .iter()
                .filter_map(|hook| {
                    let kind = match hook.key.id.as_str() {
                        "recipe_result" => crate::gui::DocHookKind::CraftRecipeResult,
                        "recipe_ingredients" => crate::gui::DocHookKind::CraftRecipeIngredients,
                        _ => return None,
                    };
                    let index = hook.key.item? as usize;
                    let rect = crate::gui::SlotRect {
                        x: hook.rect.x as f32,
                        y: hook.rect.y as f32,
                        w: hook.rect.w as f32,
                        h: hook.rect.h as f32,
                    };
                    let clip = hook.clip.map(|clip| crate::gui::SlotRect {
                        x: clip.x as f32,
                        y: clip.y as f32,
                        w: clip.w as f32,
                        h: clip.h as f32,
                    });
                    Some(crate::gui::DocHook {
                        kind,
                        index,
                        rect,
                        clip,
                    })
                })
                .collect(),
        )
    }

    /// Drop the active screen's ephemeral state (screen closed/changed).
    pub fn deactivate(&mut self) {
        if self.active.take().is_some() {
            self.fs.reset();
            self.state.clear();
            self.extra_images.clear();
            self.dynamic_images.clear();
            self.frame_stamp = None;
        }
        self.input.clear();
    }
}

#[cfg(test)]
mod frame_stamp_tests {
    use super::*;

    #[test]
    fn a_solved_document_keeps_its_generation_until_it_is_resolved_again() {
        let mut ui = AppUi::new();
        let screen = (1280, 720);
        ui.set_viewport_generation(11);
        assert!(ui.frame(GuiKind::Hotbar, screen, 0.0, None));
        let first = (GuiKind::Hotbar, crate::gui::UiViewport::new(screen, 11));
        assert_eq!(ui.frame_stamp(), Some(first));

        ui.set_viewport_generation(12);
        assert_eq!(ui.frame_stamp(), Some(first));

        assert!(ui.frame(GuiKind::Hotbar, screen, 0.1, None));
        assert_eq!(
            ui.frame_stamp(),
            Some((GuiKind::Hotbar, crate::gui::UiViewport::new(screen, 12)))
        );
    }
}

/// Sample state + event handling for the dev widget-catalog demo screen
/// (`PETRAMOND_UI_DEMO=1`) — the seam proof, not a real controller.
pub(super) mod demo {
    use petramond_ui::{UiEvent, UiMap, UiState, UiValue};
    use std::sync::Arc;

    pub fn populate(state: &mut UiState) {
        if state.get("demo_rows").is_some() {
            return;
        }
        state.set("demo_on", UiValue::Bool(true));
        state.set("never", UiValue::Bool(false));
        state.set("demo_volume", UiValue::F32(75.0));
        state.set("demo_cook", UiValue::F32(0.6));
        state.set("demo_burn", UiValue::F32(0.4));
        state.set("demo_sel", UiValue::I32(-1));
        let rows: Vec<UiMap> = [
            ("Weather Pack", "v0.1.0", true),
            ("Zombies", "v0.1.0", false),
            ("Wheel of Fortune", "v0.2.3", true),
            ("Daylight", "v0.1.1", true),
            ("Extra Row", "v0.0.9", false),
        ]
        .iter()
        .map(|(n, v, e)| {
            let mut m = UiMap::new();
            m.insert("name".into(), UiValue::Str((*n).into()));
            m.insert("version".into(), UiValue::Str((*v).into()));
            m.insert("enabled".into(), UiValue::Bool(*e));
            m
        })
        .collect();
        state.set("demo_rows", UiValue::List(Arc::new(rows)));
    }

    pub fn apply_one(state: &mut UiState, ev: &UiEvent) {
        apply(state, std::slice::from_ref(ev));
    }

    pub fn apply(state: &mut UiState, events: &[UiEvent]) {
        for ev in events {
            match ev {
                UiEvent::Toggle { id, item: None, on } if id == "t1" || id == "c1" => {
                    state.set("demo_on", UiValue::Bool(*on));
                }
                UiEvent::Toggle {
                    id,
                    item: Some(i),
                    on,
                } if id == "row_on" => {
                    if let Some(rows) = state.get_list("demo_rows").cloned() {
                        let mut rows = (*rows).clone();
                        if let Some(row) = rows.get_mut(*i as usize) {
                            row.insert("enabled".into(), UiValue::Bool(*on));
                        }
                        state.set("demo_rows", UiValue::List(Arc::new(rows)));
                    }
                }
                UiEvent::SliderChange { id, value, .. } if id == "vol" => {
                    state.set("demo_volume", UiValue::F32(*value));
                    state.set("demo_cook", UiValue::F32(*value / 100.0));
                }
                UiEvent::ListSelect { id, index } if id == "mods" => {
                    state.set("demo_sel", UiValue::I32(*index as i32));
                }
                _ => {}
            }
        }
    }
}
