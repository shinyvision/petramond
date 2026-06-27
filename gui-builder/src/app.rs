//! The eframe application: holds project + asset state, drives the panels, owns
//! File operations, undo/redo history, snap settings, the drag-gesture state
//! machine, and the layer-panel drag/rename state.

use crate::assets::AssetLibrary;
use crate::model::{AssetSpec, DropTarget, GuiType, LayerFit, LayerTag, Project, RectF};
use crate::{bake, canvas, ui};
use eframe::egui;
use std::path::PathBuf;

/// What's currently selected (by stable id).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    Layer(u64),
    Group(u64),
    Slot(u64),
}

impl Selection {
    pub fn id(self) -> u64 {
        match self {
            Selection::Layer(id) | Selection::Group(id) | Selection::Slot(id) => id,
        }
    }
}

/// An in-flight drag from the asset palette onto the canvas.
pub enum DragPayload {
    Asset { spec: AssetSpec, size: [usize; 2], default_fit: LayerFit, cover: bool },
    Slot,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DragMode {
    Move,
    Resize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Horizontal,
    Vertical,
}

/// An in-flight move/resize of a canvas item. For a group, `group_children`
/// holds each child's starting rect so the whole group moves rigidly.
pub struct DragMove {
    pub sel: Selection,
    pub mode: DragMode,
    pub start_rect: RectF,
    pub group_children: Vec<(u64, RectF)>,
    pub raw_total: egui::Vec2,
    pub shift_anchor: Option<egui::Vec2>,
    pub axis: Option<Axis>,
}

/// A row being dragged within the layer panel.
#[derive(Clone, Copy)]
pub struct PanelItem {
    pub id: u64,
    pub is_group: bool,
}

/// Open state for the "tag layer" dialog: which layer, and the pending choice.
#[derive(Clone)]
pub struct TagDialog {
    pub layer_id: u64,
    pub choice: Option<LayerTag>,
}

#[derive(Clone)]
struct Snapshot {
    project: Project,
    selection: Option<Selection>,
}

pub struct View {
    pub zoom: f32,
    pub pan: egui::Vec2,
}

pub struct App {
    pub project: Project,
    pub assets: AssetLibrary,
    pub selection: Option<Selection>,
    pub show_slots: bool,
    pub view: View,
    pub drag: Option<DragPayload>,
    pub active_drag: Option<DragMove>,
    pub panning: bool,

    // Layer-panel interaction.
    pub panel_drag: Option<PanelItem>,
    pub renaming: Option<u64>,
    pub rename_buf: String,
    pub rename_focus: bool,

    pub project_path: Option<PathBuf>,
    pub status: String,

    pub snap_enabled: bool,
    pub snap_step: i32,
    pub show_snap_dialog: bool,
    pub tag_dialog: Option<TagDialog>,

    undo_stack: Vec<Snapshot>,
    redo_stack: Vec<Snapshot>,
    pending: Option<Snapshot>,

    next_id: u64,
}

const UNDO_LIMIT: usize = 200;

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        crate::icons::install(&cc.egui_ctx);
        // Labels shouldn't be text-selectable (only inputs are); this also stops
        // dragging a layer row from highlighting its text.
        let mut style = (*cc.egui_ctx.style()).clone();
        style.interaction.selectable_labels = false;
        cc.egui_ctx.set_style(style);

        let project = Project::new(GuiType::Chest);
        let next_id = project.max_id() + 1;
        App {
            assets: AssetLibrary::new(&cc.egui_ctx),
            project,
            selection: None,
            show_slots: true,
            view: View { zoom: 1.0, pan: egui::Vec2::ZERO },
            drag: None,
            active_drag: None,
            panning: false,
            panel_drag: None,
            renaming: None,
            rename_buf: String::new(),
            rename_focus: false,
            project_path: None,
            status: "New chest GUI. Drag parts from the right; place slots; File ▾ to bake.".to_string(),
            snap_enabled: false,
            snap_step: 8,
            show_snap_dialog: false,
            tag_dialog: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            pending: None,
            next_id,
        }
    }

    pub fn alloc_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub fn selected_slot_idx(&self) -> Option<usize> {
        match self.selection {
            Some(Selection::Slot(id)) => self.project.slots.iter().position(|s| s.id == id),
            _ => None,
        }
    }

    // ---- Undo / redo -----------------------------------------------------

    fn snapshot(&self) -> Snapshot {
        Snapshot { project: self.project.clone(), selection: self.selection }
    }

    pub fn begin_edit(&mut self) {
        if self.pending.is_none() {
            self.pending = Some(self.snapshot());
        }
    }

    fn commit_edit(&mut self, pointer_down: bool) {
        if pointer_down {
            return;
        }
        if let Some(prev) = self.pending.take() {
            if prev.project != self.project {
                self.undo_stack.push(prev);
                if self.undo_stack.len() > UNDO_LIMIT {
                    self.undo_stack.remove(0);
                }
                self.redo_stack.clear();
            }
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn undo(&mut self) {
        if let Some(prev) = self.undo_stack.pop() {
            self.redo_stack.push(self.snapshot());
            self.project = prev.project;
            self.selection = prev.selection;
            self.reset_transient();
            self.status = "Undo".to_string();
        }
    }

    pub fn redo(&mut self) {
        if let Some(next) = self.redo_stack.pop() {
            self.undo_stack.push(self.snapshot());
            self.project = next.project;
            self.selection = next.selection;
            self.reset_transient();
            self.status = "Redo".to_string();
        }
    }

    fn reset_transient(&mut self) {
        self.pending = None;
        self.active_drag = None;
        self.panel_drag = None;
        self.renaming = None;
        self.rename_focus = false;
        self.tag_dialog = None;
    }

    fn clear_history(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.reset_transient();
    }

    // ---- Tree edits ------------------------------------------------------

    pub fn add_group(&mut self) {
        self.begin_edit();
        let id = self.alloc_id();
        self.project.add_group(id);
        self.selection = Some(Selection::Group(id));
        self.status = "Added group".to_string();
    }

    pub fn duplicate(&mut self, id: u64) {
        self.begin_edit();
        let start = self.next_id + 1;
        if let Some((new_id, used, is_group)) = self.project.duplicate(id, start) {
            self.next_id += used;
            self.selection = Some(if is_group { Selection::Group(new_id) } else { Selection::Layer(new_id) });
            self.status = "Duplicated".to_string();
        }
    }

    pub fn ungroup(&mut self, gid: u64) {
        self.begin_edit();
        self.project.ungroup(gid);
        self.selection = None;
        self.status = "Ungrouped".to_string();
    }

    pub fn move_panel(&mut self, id: u64, target: DropTarget) {
        self.begin_edit();
        self.project.move_item(id, target);
    }

    pub fn rename(&mut self, id: u64, name: String) {
        self.begin_edit();
        self.project.rename(id, name);
    }

    pub fn set_layer_tag(&mut self, id: u64, tag: Option<LayerTag>) {
        self.begin_edit();
        self.project.set_layer_tag(id, tag);
        self.status = match tag {
            Some(t) => format!("Tagged “{}”", t.label()),
            None => "Removed tag".to_string(),
        };
    }

    pub fn delete_selection(&mut self) {
        match self.selection {
            Some(Selection::Layer(id)) | Some(Selection::Group(id)) => {
                self.begin_edit();
                self.project.delete(id);
                self.selection = None;
                self.status = "Deleted".to_string();
            }
            Some(Selection::Slot(id)) => {
                self.begin_edit();
                self.project.slots.retain(|s| s.id != id);
                self.selection = None;
                self.status = "Deleted slot".to_string();
            }
            None => {}
        }
    }

    // ---- File operations -------------------------------------------------

    pub fn new_project(&mut self) {
        let ty = self.project.gui_type;
        self.project = Project::new(ty);
        self.next_id = self.project.max_id() + 1;
        self.selection = None;
        self.project_path = None;
        self.clear_history();
        self.status = format!("New {} GUI.", ty.label());
    }

    pub fn open(&mut self, ctx: &egui::Context) {
        let Some(path) = rfd::FileDialog::new().add_filter("Llamacraft GUI", &["llgui"]).pick_file() else {
            return;
        };
        match std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|s| serde_json::from_str::<Project>(&s).map_err(|e| e.to_string()))
        {
            Ok(project) => {
                self.project = project;
                self.ensure_assets(ctx);
                self.next_id = self.project.max_id() + 1;
                self.selection = None;
                self.clear_history();
                self.status = format!("Opened {}", path.display());
                self.project_path = Some(path);
            }
            Err(e) => self.status = format!("Open failed: {e}"),
        }
    }

    pub fn save(&mut self, ctx: &egui::Context) {
        if let Some(path) = self.project_path.clone() {
            self.write_project(&path);
        } else {
            self.save_as(ctx);
        }
    }

    pub fn save_as(&mut self, _ctx: &egui::Context) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Llamacraft GUI", &["llgui"])
            .set_file_name("untitled.llgui")
            .save_file()
        else {
            return;
        };
        let path = if path.extension().is_some() { path } else { path.with_extension("llgui") };
        self.write_project(&path);
        self.project_path = Some(path);
    }

    fn write_project(&mut self, path: &std::path::Path) {
        match serde_json::to_string_pretty(&self.project)
            .map_err(|e| e.to_string())
            .and_then(|s| std::fs::write(path, s).map_err(|e| e.to_string()))
        {
            Ok(()) => self.status = format!("Saved {}", path.display()),
            Err(e) => self.status = format!("Save failed: {e}"),
        }
    }

    pub fn bake(&mut self, _ctx: &egui::Context) {
        let stem = gui_file_stem(self.project.gui_type);
        let Some(path) = rfd::FileDialog::new()
            .add_filter("PNG", &["png"])
            .set_file_name(format!("{stem}.png"))
            .save_file()
        else {
            return;
        };
        match bake::bake_to_files(&self.project, &self.assets, &path) {
            Ok(()) => {
                let json = path.with_extension("json");
                self.status = format!("Baked {} + {}", path.display(), json.display());
            }
            Err(e) => self.status = format!("Bake failed: {e}"),
        }
    }

    fn ensure_assets(&mut self, ctx: &egui::Context) {
        let mut specs: Vec<AssetSpec> = self.project.flat_layers().iter().map(|fl| fl.layer.asset.clone()).collect();
        if let Some(h) = &self.project.hover {
            specs.push(h.asset.clone());
        }
        let mut missing = Vec::new();
        for spec in specs {
            if let Err(e) = self.assets.ensure(ctx, &spec) {
                missing.push(e);
            }
        }
        if !missing.is_empty() {
            self.status = format!("Loaded with {} missing asset(s): {}", missing.len(), missing.join("; "));
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let typing = ctx.wants_keyboard_input();
        let (save, open, new, bake, undo, redo, del, dup) = ctx.input(|i| {
            let c = i.modifiers.command;
            let shift = i.modifiers.shift;
            (
                c && i.key_pressed(egui::Key::S),
                c && i.key_pressed(egui::Key::O),
                c && i.key_pressed(egui::Key::N),
                c && i.key_pressed(egui::Key::B),
                c && !shift && i.key_pressed(egui::Key::Z),
                (c && shift && i.key_pressed(egui::Key::Z)) || (c && i.key_pressed(egui::Key::Y)),
                i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
                c && i.key_pressed(egui::Key::D),
            )
        });
        if new {
            self.new_project();
        }
        if open {
            self.open(ctx);
        }
        if save {
            self.save(ctx);
        }
        if bake {
            self.bake(ctx);
        }
        if !typing {
            if undo {
                self.undo();
            }
            if redo {
                self.redo();
            }
            if del {
                self.delete_selection();
            }
            if dup {
                if let Some(sel) = self.selection {
                    self.duplicate(sel.id());
                }
            }
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_shortcuts(ctx);

        ui::top_bar(self, ctx);
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.status);
            });
        });
        ui::right_panel(self, ctx);
        ui::left_panel(self, ctx);
        egui::CentralPanel::default().show(ctx, |ui| {
            canvas::show_canvas(self, ui);
        });
        ui::snap_dialog(self, ctx);
        ui::tag_dialog(self, ctx);

        let pointer_down = ctx.input(|i| i.pointer.any_down());
        self.commit_edit(pointer_down);
    }
}

pub fn gui_file_stem(ty: GuiType) -> &'static str {
    match ty {
        GuiType::Chest => "chest",
        GuiType::Inventory => "inventory",
        GuiType::CraftingTable => "crafting_table",
        GuiType::Furnace => "furnace",
        GuiType::Hotbar => "hotbar",
        GuiType::FurnitureWorkbench => "furniture_workbench",
        GuiType::Custom => "gui",
    }
}
