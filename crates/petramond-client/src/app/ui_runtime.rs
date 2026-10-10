use std::collections::HashMap;

use petramond::gui::{doc_theme, documents};

use petramond_ui::{DocImages, FrameArgs, FrameOutput, FrameState, InputEvent, UiRuntime, UiState};
use petramond_world::gui_state::GuiKind;
use petramond_world::item::ItemType;

struct DocImageSet<'a> {
    doc: std::sync::Arc<Vec<documents::DocImageRef>>,
    extra: &'a [documents::DocImageRef],
    dynamic: &'a [petramond::modding::ClientImageData],
    scenes: &'a std::collections::BTreeMap<String, (u64, petramond_ui::SceneView)>,
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

    fn scene(&self, name: &str) -> Option<&petramond_ui::SceneView> {
        self.scenes.get(name).map(|(_, scene)| scene)
    }
}

fn scene_view(scene: &petramond::modding::ClientCanvasSceneData) -> petramond_ui::SceneView {
    use mod_api::ClientCanvasElement as E;
    use petramond_ui::SceneElement as S;
    let rgba = |c: [u8; 4]| c.map(|v| f32::from(v) / 255.0);
    petramond_ui::SceneView {
        offset: scene.offset,
        elements: scene
            .elements
            .iter()
            .map(|element| match element {
                E::Image { image_key, rect } => S::Image {
                    image: image_key.clone(),
                    rect: *rect,
                },
                E::Sprite { image_key, center } => S::Sprite {
                    image: image_key.clone(),
                    center: *center,
                },
                E::Rect {
                    rect,
                    color,
                    filled,
                } => S::Rect {
                    rect: *rect,
                    color: rgba(*color),
                    filled: *filled,
                },
                E::Text {
                    pos,
                    text,
                    color,
                    small,
                    max_w,
                } => S::Text {
                    pos: *pos,
                    text: text.clone(),
                    color: rgba(*color),
                    small: *small,
                    max_w: *max_w,
                },
            })
            .collect(),
    }
}

pub(super) struct AppUi {
    fs: FrameState,
    out: FrameOutput,
    state: UiState,
    input: Vec<InputEvent>,
    active: Option<GuiKind>,
    extra_images: Vec<documents::DocImageRef>,
    dynamic_images: Vec<petramond::modding::ClientImageData>,
    overlaid_keys: Vec<String>,
    scenes: std::collections::BTreeMap<String, (u64, petramond_ui::SceneView)>,
    image_sources: Vec<petramond::gui::DocImageSource>,
    viewport_generation: u64,
    frame_stamp: Option<(GuiKind, petramond::gui::UiViewport)>,
    solve_serial: u64,
    geometry_serial: Option<u64>,
    doc_slots: Vec<petramond::gui::DocSlot>,
    doc_hooks: Vec<petramond::gui::DocHook>,
    item_layers: HashMap<String, Box<[(ItemType, bool)]>>,
    clipboard: Box<dyn petramond_ui::TextClipboard>,
}

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
            overlaid_keys: Vec::new(),
            scenes: Default::default(),
            image_sources: Vec::new(),
            viewport_generation: 0,
            frame_stamp: None,
            solve_serial: 0,
            geometry_serial: None,
            doc_slots: Vec::new(),
            doc_hooks: Vec::new(),
            item_layers: HashMap::new(),
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

    pub fn doc_backed(kind: GuiKind) -> bool {
        documents::doc_for(kind).is_some()
    }

    pub fn push_input(&mut self, ev: InputEvent) {
        self.input.push(ev);
    }

    pub fn state_mut(&mut self) -> &mut UiState {
        &mut self.state
    }

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

    pub fn set_dynamic_images(&mut self, images: Vec<petramond::modding::ClientImageData>) {
        self.dynamic_images = images;
    }

    pub fn set_scenes(&mut self, scenes: &[(String, petramond::modding::ClientCanvasSceneData)]) {
        self.scenes
            .retain(|key, _| scenes.iter().any(|(k, _)| k == key));
        for (key, scene) in scenes {
            if self.scenes.get(key).map(|(rev, _)| *rev) != Some(scene.revision) {
                self.scenes
                    .insert(key.clone(), (scene.revision, scene_view(scene)));
            }
        }
    }

    pub fn replace_client_state(
        &mut self,
        state: &std::collections::BTreeMap<String, mod_api::GuiValue>,
    ) {
        self.state.clear();
        self.overlaid_keys.clear();
        self.overlay_client_state(state);
    }

    /// Lays a client instance's UI state over what the screen already holds, and
    /// withdraws the keys a previous overlay set that this one no longer carries.
    pub fn overlay_client_state(
        &mut self,
        state: &std::collections::BTreeMap<String, mod_api::GuiValue>,
    ) {
        for key in self.overlaid_keys.drain(..) {
            if !state.contains_key(&key) {
                self.state.remove(&key);
            }
        }
        for (key, value) in state {
            let value = super::gui_value::from_api(value);
            self.state.set(key.clone(), value);
        }
        self.overlaid_keys.extend(state.keys().cloned());
    }

    pub fn image_sources(&self) -> &[petramond::gui::DocImageSource] {
        &self.image_sources
    }

    pub fn text_input_focused(&self) -> bool {
        self.fs.focused().is_some()
    }

    pub fn set_viewport_generation(&mut self, generation: u64) {
        self.viewport_generation = generation;
    }

    pub fn frame_stamp(&self) -> Option<(GuiKind, petramond::gui::UiViewport)> {
        self.frame_stamp
    }

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

    pub fn request_focus(&mut self, key: petramond_ui::InstKey) {
        self.fs.request_focus(key);
    }

    pub fn ensure_active(&mut self, kind: GuiKind) {
        if self.active != Some(kind) {
            self.reset_screen_state();
            self.active = Some(kind);
        }
    }

    fn reset_screen_state(&mut self) {
        self.fs.reset();
        self.state.clear();
        self.overlaid_keys.clear();
        self.extra_images.clear();
        self.dynamic_images.clear();
        self.scenes.clear();
        self.frame_stamp = None;
        self.out.hover_slot = None;
        self.out.hover_item = None;
        self.item_layers.clear();
    }

    pub fn frame(
        &mut self,
        kind: GuiKind,
        screen: (u32, u32),
        now: f64,
        dim: Option<[f32; 4]>,
    ) -> bool {
        let viewport = petramond::gui::UiViewport::new(screen, self.viewport_generation);
        self.frame_in(kind, viewport, now, dim)
    }

    pub fn frame_in(
        &mut self,
        kind: GuiKind,
        viewport: petramond::gui::UiViewport,
        now: f64,
        dim: Option<[f32; 4]>,
    ) -> bool {
        let Some(doc) = documents::doc_for(kind) else {
            self.input.clear();
            self.frame_stamp = None;
            return false;
        };
        self.ensure_active(kind);
        let rt = UiRuntime::new(doc.doc, doc_theme::theme());
        self.image_sources.clear();
        self.image_sources.extend(
            doc.images
                .iter()
                .map(|i| petramond::gui::DocImageSource::Path(i.path.clone())),
        );
        self.image_sources.extend(
            self.extra_images
                .iter()
                .map(|i| petramond::gui::DocImageSource::Path(i.path.clone())),
        );
        self.image_sources
            .extend(
                self.dynamic_images
                    .iter()
                    .map(|i| petramond::gui::DocImageSource::Dynamic {
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
            scenes: &self.scenes,
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
        self.solve_serial = self.solve_serial.wrapping_add(1);
        self.frame_stamp = Some((kind, viewport));
        true
    }

    pub fn take_events(&mut self) -> Vec<petramond_ui::UiEvent> {
        std::mem::take(&mut self.out.events)
    }

    pub fn out(&self) -> &FrameOutput {
        &self.out
    }

    pub fn hover_item(&self, id: &str) -> Option<usize> {
        self.out
            .hover_item
            .as_ref()
            .filter(|(list, _)| list == id)
            .map(|(_, item)| *item as usize)
    }

    pub fn menu_slot_at(&self, x: f32, y: f32) -> Option<petramond_world::gui_state::MenuSlot> {
        let (_, viewport) = self.frame_stamp?;
        if viewport.generation != self.viewport_generation {
            return None;
        }
        self.out.slots.iter().rev().find_map(|slot| {
            let rect = slot.rect;
            let inside = x >= rect.x as f32
                && x < (rect.x + rect.w) as f32
                && y >= rect.y as f32
                && y < (rect.y + rect.h) as f32;
            if !inside {
                return None;
            }
            petramond::gui::Role::from_key(&slot.role)
                .and_then(|role| role.menu_slot(slot.index as usize))
        })
    }

    pub fn menu_drag_preview(
        &self,
    ) -> Option<(
        Vec<petramond_world::gui_state::MenuSlot>,
        petramond_ui::PointerButton,
    )> {
        let (_, viewport) = self.frame_stamp?;
        if viewport.generation != self.viewport_generation {
            return None;
        }
        let (button, slots) = self.fs.slot_drag()?;
        let slots: Vec<_> = slots
            .iter()
            .take(petramond_world::gui_state::MAX_MENU_DRAG_SLOTS)
            .filter_map(|(role, index)| {
                petramond::gui::Role::from_key(role)
                    .and_then(|role| role.menu_slot(*index as usize))
            })
            .collect();
        (!slots.is_empty()).then_some((slots, button))
    }

    pub fn refresh_doc_geometry(&mut self) {
        if self.geometry_serial == Some(self.solve_serial) {
            return;
        }
        self.geometry_serial = Some(self.solve_serial);
        self.doc_slots.clear();
        self.doc_slots.extend(self.out.slots.iter().filter_map(|s| {
            let role = petramond::gui::Role::from_key(&s.role)?;
            let mut slot = petramond::gui::DocSlot::new(role, s.index, slot_rect(s.rect));
            slot.raised = s.raised;
            Some(slot)
        }));
        self.doc_hooks.clear();
        for hook in &self.out.hooks {
            let rect = slot_rect(hook.rect);
            let clip = hook.clip.map(slot_rect);
            let doc_hook = |kind, index| petramond::gui::DocHook {
                kind,
                index,
                rect,
                clip,
                overlay: hook.overlay,
            };
            // An `item`-bound hook is generic (any document, any id): its
            // bound value is an ordered comma-list of item names (the
            // `petramond:overlay` convention), one composited layer per name,
            // first name bottom-most; a `~` prefix marks that layer a GHOST
            // (drawn dimmed). Unknown names contribute nothing — lenient like
            // every by-reference item read. The bare ids are the crafting
            // browser's bespoke hooks.
            if let Some(names) = hook.item.as_deref() {
                if !self.item_layers.contains_key(names) {
                    self.item_layers
                        .insert(names.to_owned(), parse_item_layers(names));
                }
                self.doc_hooks
                    .extend(self.item_layers[names].iter().map(|&(item, dim)| {
                        doc_hook(petramond::gui::DocHookKind::ItemView { item, dim }, 0)
                    }));
                continue;
            }
            let hook_view = match hook.key.id.as_str() {
                "recipe_result" => hook
                    .key
                    .item
                    .map(|row| doc_hook(petramond::gui::DocHookKind::RecipeResult, row as usize)),
                "craft_tip_result" => Some(doc_hook(petramond::gui::DocHookKind::TipResult, 0)),
                "craft_tip_ingredients" => {
                    Some(doc_hook(petramond::gui::DocHookKind::TipIngredients, 0))
                }
                _ => None,
            };
            self.doc_hooks.extend(hook_view);
        }
    }

    pub fn doc_geometry(&self) -> (&[petramond::gui::DocSlot], &[petramond::gui::DocHook]) {
        (&self.doc_slots, &self.doc_hooks)
    }

    pub fn deactivate(&mut self) {
        if self.active.take().is_some() {
            self.reset_screen_state();
        }
        self.input.clear();
    }
}

fn slot_rect(r: petramond_ui::RectI) -> petramond::gui::SlotRect {
    petramond::gui::SlotRect {
        x: r.x as f32,
        y: r.y as f32,
        w: r.w as f32,
        h: r.h as f32,
    }
}

fn parse_item_layers(names: &str) -> Box<[(ItemType, bool)]> {
    names
        .split(',')
        .filter_map(|name| {
            let (name, dim) = item_view_layer(name);
            Some((ItemType::by_name(name)?, dim))
        })
        .collect()
}

fn item_view_layer(name: &str) -> (&str, bool) {
    let name = name.trim();
    match name.strip_prefix('~') {
        Some(bare) => (bare.trim_start(), true),
        None => (name, false),
    }
}

#[cfg(test)]
mod frame_stamp_tests {
    use super::*;

    #[test]
    fn an_item_view_layers_ghost_marker_is_the_tilde_prefix() {
        assert_eq!(
            item_view_layer("petramond:diamond"),
            ("petramond:diamond", false)
        );
        assert_eq!(
            item_view_layer("~petramond:diamond"),
            ("petramond:diamond", true)
        );
        assert_eq!(
            item_view_layer(" ~ petramond:diamond "),
            ("petramond:diamond", true)
        );
        assert_eq!(item_view_layer(""), ("", false));
    }

    #[test]
    fn doc_geometry_is_derived_once_per_solve() {
        let stone = ItemType::by_name("petramond:stone").expect("stone is registered");
        let dirt = ItemType::by_name("petramond:dirt").expect("dirt is registered");
        let mut ui = AppUi::new();
        ui.out.hooks.push(petramond_ui::HookRectOut {
            key: petramond_ui::InstKey {
                id: "subject".into(),
                item: None,
            },
            rect: petramond_ui::RectI {
                x: 1,
                y: 2,
                w: 3,
                h: 4,
            },
            clip: None,
            overlay: false,
            item: Some("petramond:stone, ~petramond:no_such_item, ~petramond:dirt".into()),
        });
        ui.refresh_doc_geometry();
        let kinds: Vec<_> = ui.doc_geometry().1.iter().map(|hook| hook.kind).collect();
        assert_eq!(
            kinds,
            [
                petramond::gui::DocHookKind::ItemView {
                    item: stone,
                    dim: false
                },
                petramond::gui::DocHookKind::ItemView {
                    item: dirt,
                    dim: true
                },
            ]
        );

        ui.out.hooks.clear();
        ui.refresh_doc_geometry();
        assert_eq!(ui.doc_geometry().1.len(), 2);
        assert_eq!(ui.item_layers.len(), 1, "one distinct list, parsed once");

        assert!(ui.frame(GuiKind::Hotbar, (1280, 720), 0.0, None));
        ui.out.hooks.clear();
        ui.refresh_doc_geometry();
        assert!(ui.doc_geometry().1.is_empty());
    }

    #[test]
    fn a_solved_document_keeps_its_generation_until_it_is_resolved_again() {
        let mut ui = AppUi::new();
        let screen = (1280, 720);
        ui.set_viewport_generation(11);
        assert!(ui.frame(GuiKind::Hotbar, screen, 0.0, None));
        let first = (GuiKind::Hotbar, petramond::gui::UiViewport::new(screen, 11));
        assert_eq!(ui.frame_stamp(), Some(first));

        ui.set_viewport_generation(12);
        assert_eq!(ui.frame_stamp(), Some(first));

        assert!(ui.frame(GuiKind::Hotbar, screen, 0.1, None));
        assert_eq!(
            ui.frame_stamp(),
            Some((GuiKind::Hotbar, petramond::gui::UiViewport::new(screen, 12)))
        );
    }
}

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
                UiEvent::Toggle {
                    id, item: None, on, ..
                } if id == "t1" || id == "c1" => {
                    state.set("demo_on", UiValue::Bool(*on));
                }
                UiEvent::Toggle {
                    id,
                    item: Some(i),
                    on,
                    ..
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
