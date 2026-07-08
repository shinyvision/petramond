//! The eframe application: menu/toolbar chrome, dock layout, selection +
//! undo plumbing. Panels and the canvas live in their own modules and talk to
//! the app through `App`'s small mutation API so every edit lands in history.

use crate::bindings::{self, Catalog};
use crate::doc_edit::{self, NodePath};
use crate::history::History;
use crate::panels;
use crate::preview::DiskImages;
use crate::project::Project;
use crate::theme_src::{self, ThemeSource};
use crate::{canvas, io, theme_bar};
use eframe::egui;
use petramond_ui::{DocIssue, UiState};
use std::path::PathBuf;

/// Forced widget states for the preview (applied to the selected node).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Forced {
    pub hover: bool,
    pub pressed: bool,
    pub focus: bool,
}

pub struct App {
    pub proj: Project,
    pub path: Option<PathBuf>,
    pub last_dir: Option<PathBuf>,
    pub dirty: bool,
    /// Bumped whenever anything the preview reads changes (doc, sample state,
    /// theme reload…). The canvas re-rasterizes only when this moves.
    pub doc_rev: u64,
    pub history: History,
    pub sel: Option<NodePath>,
    /// External (non-tree) selection happened: the tree scrolls the selected
    /// row into view once.
    pub tree_scroll_to_sel: bool,
    /// Collapsed container rows in the doc tree (session-only; paths shift
    /// with edits, which just re-expands moved rows).
    pub tree_collapsed: std::collections::HashSet<NodePath>,
    pub theme: ThemeSource,
    /// The per-kind data catalog (`assets/ui/bindings.json`); `None` hides
    /// binding pickers, seeding, and the Screen-data panel.
    pub catalog: Option<Catalog>,
    /// `Some(true)` forces the Screen-data header open once (new documents).
    pub screen_data_force_open: Option<bool>,
    pub images: DiskImages,
    pub forced: Forced,
    /// Editor chrome (selection outlines, badges) on the canvas.
    pub overlay: bool,
    // Sample-state JSON editor buffer.
    pub sample_json: String,
    pub sample_error: Option<String>,
    pub sample_open: bool,
    // New-with-custom-kind popup.
    pub new_kind_open: bool,
    pub new_kind_buf: String,
    // Cached validation.
    validation: Vec<DocIssue>,
    validation_rev: Option<u64>,
    /// Set by canvas double-click: the inspector focuses its text field.
    pub focus_text_edit: bool,
    pub status: String,
    pub canvas_drag: Option<canvas::CanvasDrag>,
    /// Cached preview raster (re-rasterized only when `canvas_tex_key` moves).
    pub canvas_tex: Option<egui::TextureHandle>,
    pub canvas_tex_key: Option<canvas::CanvasKey>,
    last_title: String,
}

impl App {
    pub fn new(_cc: &eframe::CreationContext<'_>, open: Option<PathBuf>) -> App {
        let mut app = App {
            proj: Project::new("petramond:pause"),
            path: None,
            last_dir: None,
            dirty: false,
            doc_rev: 0,
            history: History::new(),
            sel: None,
            tree_scroll_to_sel: false,
            tree_collapsed: std::collections::HashSet::new(),
            theme: theme_src::load(0),
            catalog: Catalog::load(),
            screen_data_force_open: None,
            images: DiskImages::empty(),
            forced: Forced::default(),
            overlay: true,
            sample_json: String::new(),
            sample_error: None,
            sample_open: false,
            new_kind_open: false,
            new_kind_buf: String::new(),
            validation: Vec::new(),
            validation_rev: None,
            focus_text_edit: false,
            status: "Ready".into(),
            canvas_drag: None,
            canvas_tex: None,
            canvas_tex_key: None,
            last_title: String::new(),
        };
        app.sync_sample_buffer();
        if let Some(path) = open {
            app.open_path(path);
        }
        app
    }

    // ---- mutation API (everything routes undo through here) -------------------

    /// A discrete document edit: snapshots for undo, then applies `f`.
    pub fn mutate(&mut self, f: impl FnOnce(&mut petramond_ui::Document)) {
        self.history.record(&self.proj.document);
        f(&mut self.proj.document);
        self.touch();
    }

    /// Start (or continue) a coalescing gesture edit, then apply `f`.
    pub fn gesture_mutate(&mut self, f: impl FnOnce(&mut petramond_ui::Document)) {
        self.history.begin_gesture(&self.proj.document);
        f(&mut self.proj.document);
        self.touch();
    }

    /// Mark preview inputs changed (also used for non-document changes like
    /// sample state or preview settings).
    pub fn touch(&mut self) {
        self.doc_rev += 1;
        self.dirty = true;
    }

    pub fn undo(&mut self) {
        if self.history.undo(&mut self.proj.document) {
            self.after_history_jump();
        }
    }

    pub fn redo(&mut self) {
        if self.history.redo(&mut self.proj.document) {
            self.after_history_jump();
        }
    }

    fn after_history_jump(&mut self) {
        self.doc_rev += 1;
        self.dirty = true;
        // Selection may point at a node that no longer exists.
        if let Some(sel) = &self.sel {
            if doc_edit::node_at(&self.proj.document.root, sel).is_none() {
                self.sel = None;
            }
        }
    }

    /// Select from outside the tree panel (canvas, validation): also scrolls
    /// the tree row into view so both views stay in sync.
    pub fn select_external(&mut self, path: Option<NodePath>) {
        self.sel = path;
        self.tree_scroll_to_sel = true;
    }

    pub fn selected_node(&self) -> Option<&petramond_ui::Node> {
        doc_edit::node_at(&self.proj.document.root, self.sel.as_deref()?)
    }

    /// The preview `UiState`: the project's sample state plus non-destructive
    /// catalog seeds for every key the author hasn't set — binding a list to
    /// `worlds` shows rows immediately, without dirtying the file.
    pub fn preview_state(&self) -> UiState {
        let (mut state, _) = self.proj.sample_ui_state();
        if let Some(info) = self.catalog.as_ref().and_then(|c| c.kind(&self.proj.document.kind)) {
            bindings::apply_seeds(&mut state, info);
        }
        state
    }

    /// Cached validation for the current document + theme.
    pub fn validation(&mut self) -> Vec<DocIssue> {
        if self.validation_rev != Some(self.doc_rev) {
            let contract = crate::contracts::contract_for(&self.proj.document.kind);
            self.validation = self
                .proj
                .document
                .validate(Some(self.theme.theme.as_ref()), Some(&contract));
            // Builder-side extra: static images must resolve beside the
            // project or they draw nothing, in game and preview alike.
            let dir = self.path.as_ref().and_then(|p| p.parent().map(PathBuf::from));
            self.validation.extend(doc_edit::missing_image_issues(
                &self.proj.document,
                &|name| dir.as_ref().is_some_and(|d| d.join(name).is_file()),
            ));
            self.validation_rev = Some(self.doc_rev);
        }
        self.validation.clone()
    }

    // ---- file ops --------------------------------------------------------------

    fn set_project(&mut self, proj: Project, path: Option<PathBuf>, status: String) {
        self.proj = proj;
        self.path = path.clone();
        if let Some(p) = path {
            self.last_dir = p.parent().map(PathBuf::from);
        }
        self.dirty = false;
        self.doc_rev += 1;
        self.history.clear();
        self.sel = None;
        self.tree_collapsed.clear();
        self.canvas_drag = None;
        self.sync_sample_buffer();
        self.status = status;
    }

    pub fn new_project(&mut self, kind: &str) {
        let mut proj = Project::new(kind);
        // New documents persist their catalog seeds in sample_state so the
        // file documents its own preview data.
        if let Some(info) = self.catalog.as_ref().and_then(|c| c.kind(kind)) {
            for (key, value) in bindings::seed_values(info) {
                proj.editor
                    .sample_state
                    .entry(key)
                    .or_insert_with(|| crate::project::value_to_json(&value));
            }
        }
        self.set_project(proj, None, format!("New {kind} document"));
        self.dirty = true;
        // Teach the author what this screen can do.
        self.screen_data_force_open = Some(true);
    }

    pub fn open_path(&mut self, path: PathBuf) {
        match io::load_project(&path) {
            Ok(p) => self.set_project(p, Some(path.clone()), format!("Opened {}", path.display())),
            Err(e) => self.status = e,
        }
    }

    fn open_dialog(&mut self) {
        if let Some(path) = io::pick_open(&self.last_dir) {
            self.open_path(path);
        }
    }

    fn save(&mut self) {
        match &self.path {
            Some(path) => {
                let path = path.clone();
                match io::save_project(&path, &self.proj) {
                    Ok(()) => {
                        self.dirty = false;
                        // The project dir may have just come into existence
                        // (save-as): image warnings must re-check against it.
                        self.validation_rev = None;
                        self.status = format!("Saved {}", path.display());
                    }
                    Err(e) => self.status = e,
                }
            }
            None => self.save_as(),
        }
    }

    fn save_as(&mut self) {
        let name = format!(
            "{}.llgui",
            self.proj.document.kind.split(':').last().unwrap_or("gui")
        );
        if let Some(path) = io::pick_save(&self.last_dir, &name) {
            self.path = Some(path);
            self.save();
        }
    }

    fn import_legacy(&mut self) {
        let Some(path) = io::pick_open(&self.last_dir) else { return };
        match io::import_legacy(&path) {
            Ok((proj, warnings)) => {
                let n = warnings.len();
                for w in &warnings {
                    eprintln!("import: {w}");
                }
                self.set_project(
                    proj,
                    None,
                    format!("Imported {} ({n} TODO items — see stderr)", path.display()),
                );
                self.dirty = true;
            }
            Err(e) => self.status = e,
        }
    }

    fn export(&mut self) {
        if let Some(path) = io::pick_export(&self.proj.document, &self.last_dir) {
            let images_from = self.path.as_ref().and_then(|p| p.parent().map(PathBuf::from));
            match io::export_document(&path, &self.proj.document, images_from.as_deref()) {
                Ok(copied) => {
                    self.status = format!(
                        "Exported {}{}",
                        path.display(),
                        if copied > 0 { format!(" (+{copied} images)") } else { String::new() }
                    );
                }
                Err(e) => self.status = e,
            }
        }
    }

    // ---- sample state ------------------------------------------------------------

    pub fn sync_sample_buffer(&mut self) {
        self.sample_json =
            serde_json::to_string_pretty(&self.proj.editor.sample_state).unwrap_or_default();
        self.sample_error = None;
    }

    pub fn apply_sample_buffer(&mut self) {
        match serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&self.sample_json)
        {
            Ok(map) => {
                self.proj.editor.sample_state = map;
                self.sample_error = None;
                // Surface tagged-value errors immediately.
                let (_, errs) = self.proj.sample_ui_state();
                if !errs.is_empty() {
                    self.sample_error = Some(errs.join("; "));
                }
                self.touch();
            }
            Err(e) => self.sample_error = Some(e.to_string()),
        }
    }

    // ---- edit actions ---------------------------------------------------------------

    pub fn delete_selected(&mut self) {
        let Some(path) = self.sel.clone() else { return };
        if path.is_empty() {
            self.status = "Cannot delete the root node".into();
            return;
        }
        self.mutate(|doc| {
            doc_edit::remove_at(&mut doc.root, &path);
        });
        self.sel = None;
    }

    pub fn duplicate_selected(&mut self) {
        let Some(path) = self.sel.clone() else { return };
        if path.is_empty() {
            return;
        }
        let Some(node) = doc_edit::node_at(&self.proj.document.root, &path) else { return };
        let mut copy = node.clone();
        doc_edit::uniquify_ids(&self.proj.document, &mut copy);
        let (last, parent) = path.split_last().unwrap();
        let parent = parent.to_vec();
        let index = *last + 1;
        self.mutate(|doc| {
            doc_edit::insert_at(&mut doc.root, &parent, index, copy);
        });
        let mut new_path = parent;
        new_path.push(index);
        self.sel = Some(new_path);
    }

    /// Insert `node` into the selected container (or the selection's parent,
    /// or the root) and select it.
    pub fn insert_node(&mut self, mut node: petramond_ui::Node) {
        doc_edit::uniquify_ids(&self.proj.document, &mut node);
        let target: NodePath = match &self.sel {
            Some(path) => match doc_edit::node_at(&self.proj.document.root, path) {
                Some(n) if n.kind.is_container() => path.clone(),
                Some(_) if !path.is_empty() => path[..path.len() - 1].to_vec(),
                _ => Vec::new(),
            },
            None => Vec::new(),
        };
        let mut new_path = None;
        self.mutate(|doc| {
            let index = doc_edit::node_at(&doc.root, &target)
                .map(|n| n.children.len())
                .unwrap_or(0);
            new_path = doc_edit::insert_at(&mut doc.root, &target, index, node);
        });
        if new_path.is_some() {
            self.sel = new_path;
        }
    }

    // ---- frame ------------------------------------------------------------------------

    fn shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        const UNDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::Z);
        const REDO: KeyboardShortcut =
            KeyboardShortcut::new(Modifiers::CTRL.plus(Modifiers::SHIFT), Key::Z);
        const SAVE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::S);
        const DUP: KeyboardShortcut = KeyboardShortcut::new(Modifiers::CTRL, Key::D);
        if ctx.input_mut(|i| i.consume_shortcut(&REDO)) {
            self.redo();
        }
        if ctx.input_mut(|i| i.consume_shortcut(&UNDO)) {
            self.undo();
        }
        if ctx.input_mut(|i| i.consume_shortcut(&SAVE)) {
            self.save();
        }
        if !ctx.wants_keyboard_input() {
            if ctx.input_mut(|i| i.consume_shortcut(&DUP)) {
                self.duplicate_selected();
            }
            if ctx.input(|i| i.key_pressed(Key::Delete)) {
                self.delete_selected();
            }
        }
    }

    fn menu_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    ui.menu_button("New", |ui| {
                        for kind in crate::contracts::ENGINE_KINDS {
                            if ui.button(*kind).clicked() {
                                self.new_project(kind);
                                ui.close_menu();
                            }
                        }
                        ui.separator();
                        if ui.button("Mod kind…").clicked() {
                            self.new_kind_open = true;
                            ui.close_menu();
                        }
                    });
                    if ui.button("Open…").clicked() {
                        self.open_dialog();
                        ui.close_menu();
                    }
                    ui.menu_button("Open Sample", |ui| {
                        let samples = io::list_samples();
                        if samples.is_empty() {
                            ui.label(
                                egui::RichText::new("none — run `gui-builder --make-samples`")
                                    .weak(),
                            );
                        }
                        for (stem, path) in samples {
                            if ui.button(&stem).clicked() {
                                self.open_path(path);
                                ui.close_menu();
                            }
                        }
                    });
                    ui.separator();
                    if ui.button("Save").clicked() {
                        self.save();
                        ui.close_menu();
                    }
                    if ui.button("Save As…").clicked() {
                        self.save_as();
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Import Legacy .llgui…").clicked() {
                        self.import_legacy();
                        ui.close_menu();
                    }
                    if ui.button("Export .gui.json…").clicked() {
                        self.export();
                        ui.close_menu();
                    }
                });
                ui.menu_button("Edit", |ui| {
                    if ui
                        .add_enabled(self.history.can_undo(), egui::Button::new("Undo"))
                        .clicked()
                    {
                        self.undo();
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(self.history.can_redo(), egui::Button::new("Redo"))
                        .clicked()
                    {
                        self.redo();
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Duplicate").clicked() {
                        self.duplicate_selected();
                        ui.close_menu();
                    }
                    if ui.button("Delete").clicked() {
                        self.delete_selected();
                        ui.close_menu();
                    }
                });
                ui.menu_button("View", |ui| {
                    for z in [1.0f32, 2.0, 3.0, 4.0, 6.0, 8.0] {
                        if ui.button(format!("Zoom {z}x")).clicked() {
                            self.proj.editor.zoom = z;
                            ui.close_menu();
                        }
                    }
                    ui.separator();
                    ui.checkbox(&mut self.proj.editor.pixel_grid, "Pixel grid");
                    ui.checkbox(&mut self.overlay, "Editor overlay");
                });
                ui.separator();
                ui.label(
                    egui::RichText::new(format!(
                        "{}{}",
                        self.proj.document.kind,
                        if self.dirty { " *" } else { "" }
                    ))
                    .weak(),
                );
            });
        });
    }

    fn popups(&mut self, ctx: &egui::Context) {
        if self.new_kind_open {
            let mut open = self.new_kind_open;
            let mut create = false;
            egui::Window::new("New mod-kind document")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("Kind key (modid:name):");
                    ui.text_edit_singleline(&mut self.new_kind_buf);
                    let ok = self.new_kind_buf.contains(':');
                    if ui.add_enabled(ok, egui::Button::new("Create")).clicked() {
                        create = true;
                    }
                });
            self.new_kind_open = open;
            if create {
                let kind = self.new_kind_buf.clone();
                self.new_project(&kind);
                self.new_kind_open = false;
            }
        }
        if self.sample_open {
            let mut open = self.sample_open;
            egui::Window::new("Sample state (tagged JSON)")
                .open(&mut open)
                .default_width(420.0)
                .show(ctx, |ui| {
                    ui.label("Seeds the preview UiState. Values are tagged:");
                    ui.monospace(r#"{"volume": {"f32": 75.0}, "on": {"bool": true}}"#);
                    if ui
                        .button("Insert example list entry")
                        .on_hover_text("Appends a 'rows' list two documents' list bindings can read")
                        .clicked()
                    {
                        let mut row = petramond_ui::UiMap::new();
                        row.insert("name".into(), petramond_ui::UiValue::Str("Example".into()));
                        row.insert("version".into(), petramond_ui::UiValue::Str("v0.1.0".into()));
                        row.insert("enabled".into(), petramond_ui::UiValue::Bool(true));
                        let list = petramond_ui::UiValue::List(std::sync::Arc::new(vec![row]));
                        self.proj
                            .editor
                            .sample_state
                            .insert("rows".into(), crate::project::value_to_json(&list));
                        self.sync_sample_buffer();
                        self.touch();
                    }
                    egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.sample_json)
                                .code_editor()
                                .desired_width(f32::INFINITY)
                                .desired_rows(12),
                        );
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Apply").clicked() {
                            self.apply_sample_buffer();
                        }
                        if ui.button("Revert").clicked() {
                            self.sync_sample_buffer();
                        }
                    });
                    if let Some(err) = &self.sample_error {
                        ui.colored_label(egui::Color32::from_rgb(230, 110, 100), err.clone());
                    }
                });
            self.sample_open = open;
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.shortcuts(ctx);
        self.menu_bar(ctx);
        theme_bar::show(self, ctx);
        self.popups(ctx);

        egui::TopBottomPanel::bottom("validation")
            .resizable(true)
            .default_height(130.0)
            .show(ctx, |ui| {
                if self.catalog.is_some() {
                    ui.columns(2, |cols| {
                        panels::validation::show(self, &mut cols[0]);
                        panels::screen_data::show(self, &mut cols[1]);
                    });
                } else {
                    panels::validation::show(self, ui);
                }
            });
        egui::SidePanel::left("tree")
            .resizable(true)
            .default_width(260.0)
            .show(ctx, |ui| {
                let h = ui.available_height();
                egui::ScrollArea::vertical()
                    .id_salt("tree_scroll")
                    .max_height(h * 0.6)
                    .auto_shrink([false, false])
                    .show(ui, |ui| panels::doc_tree::show(self, ui));
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt("palette_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| panels::palette::show(self, ui));
            });
        egui::SidePanel::right("inspector")
            .resizable(true)
            .default_width(300.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| panels::inspector::show(self, ui));
            });
        egui::CentralPanel::default().show(ctx, |ui| canvas::show(self, ui));

        // Gestures end when the pointer releases, wherever it is.
        if ctx.input(|i| !i.pointer.any_down()) {
            self.history.end_gesture(&self.proj.document);
            self.canvas_drag = None;
        }

        // Keep doc images fresh (the project dir may gain PNGs while open).
        let dir = self.path.as_ref().and_then(|p| p.parent().map(PathBuf::from));
        self.images.refresh(&self.proj.document, dir.as_deref());

        let title = format!(
            "GUI Builder — {}{}",
            self.path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| self.proj.document.kind.clone()),
            if self.dirty { " *" } else { "" }
        );
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
    }
}
