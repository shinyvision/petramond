//! The app's GUI-document driver: owns the per-open-screen ephemeral widget
//! state ([`FrameState`]), queues host input as [`InputEvent`]s, runs one
//! [`UiRuntime`] frame per draw, and hands the resulting events to whoever
//! owns the screen (shell controllers app-side; slot clicks latch to the
//! tick).
//!
//! Nothing here touches `Game`/`World` — ephemeral widget state can never
//! leak into the deterministic tick. The only tick-bound artifact is a
//! [`UiEvent`] the caller explicitly latches.

use crate::gui::{doc_theme, documents, gui_scale, GuiKind};
use petramond_ui::{DocImages, FrameArgs, FrameOutput, FrameState, InputEvent, UiRuntime, UiState};

/// A document's image registry: document-local images first, then the
/// host-registered extras (controller-provided icons), in one
/// `TexId::DocImage` index space (the renderer uploads the same order).
struct DocImageSet<'a> {
    doc: std::sync::Arc<Vec<documents::DocImageRef>>,
    extra: &'a [documents::DocImageRef],
}

impl DocImages for DocImageSet<'_> {
    fn resolve(&self, name: &str) -> Option<(u16, (u32, u32))> {
        if let Some(idx) = self.doc.iter().position(|i| i.name == name) {
            return Some((idx as u16, self.doc[idx].size));
        }
        self.extra
            .iter()
            .position(|i| i.name == name)
            .map(|idx| ((self.doc.len() + idx) as u16, self.extra[idx].size))
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
    /// This frame's `DocImage` index → path order (renderer upload).
    image_paths: Vec<std::path::PathBuf>,
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
            image_paths: Vec::new(),
            clipboard: Box::new(DocClipboard::default()),
        }
    }

    #[cfg(test)]
    pub fn set_clipboard(&mut self, clipboard: Box<dyn petramond_ui::TextClipboard>) {
        self.clipboard = clipboard;
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

    /// This frame's `TexId::DocImage` index → image path order.
    pub fn image_paths(&self) -> &[std::path::PathBuf] {
        &self.image_paths
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
            return false;
        };
        self.ensure_active(kind);
        let rt = UiRuntime::new(doc.doc, doc_theme::theme());
        self.image_paths.clear();
        self.image_paths
            .extend(doc.images.iter().map(|i| i.path.clone()));
        self.image_paths
            .extend(self.extra_images.iter().map(|i| i.path.clone()));
        let images = DocImageSet {
            doc: doc.images,
            extra: &self.extra_images,
        };
        let input = std::mem::take(&mut self.input);
        rt.frame(
            FrameArgs {
                screen,
                scale: gui_scale(screen) as i32,
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

    /// Drop the active screen's ephemeral state (screen closed/changed).
    pub fn deactivate(&mut self) {
        if self.active.take().is_some() {
            self.fs.reset();
            self.state.clear();
            self.extra_images.clear();
        }
        self.input.clear();
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
