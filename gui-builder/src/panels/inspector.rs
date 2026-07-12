//! Inspector panel: edit the selected node's id, type-specific properties,
//! layout, style, and bindings. Edits are applied to a clone of the node and
//! committed through the app's undo-aware mutation API; drag-value edits ride
//! a coalescing gesture so a slider scrub is one undo entry.

use crate::app::App;
use crate::bindings::{field_matches, BindField};
use crate::doc_edit;
use eframe::egui::{self, DragValue, Response, Ui};
// NOTE: `ImageFit` isn't re-exported from the petramond_ui root (unlike its
// sibling doc types) — pulled from `doc` directly; worth adding upstream.
use petramond_ui::doc::ImageFit;
use petramond_ui::{
    AbsPos, Align, AlertLevel, Anchor, AnchorEdge, Dir, DocClass, GaugeMode, Justify, NodeKind,
    ScrollAxis, Size,
};

/// Catalog-fed options for the binding pickers: per-field global state keys
/// (with doc tooltips) plus — inside a list template — the item's own fields.
#[derive(Default)]
struct BindOptions {
    /// `(key, doc)` per bind field, already type-filtered.
    global: Vec<(BindField, Vec<(String, String)>)>,
    /// `(field name, type)` of the enclosing list's item map.
    item_fields: Vec<(String, String)>,
}

impl BindOptions {
    fn build(app: &App, path: &[usize]) -> Option<BindOptions> {
        let info = app.catalog.as_ref()?.kind(&app.proj.document.kind)?;
        let fields = [
            BindField::Text,
            BindField::Value,
            BindField::Enabled,
            BindField::Visible,
            BindField::Items,
            BindField::Selected,
            BindField::Image,
        ];
        let global = fields
            .iter()
            .map(|&f| {
                (
                    f,
                    info.keys_for(f)
                        .into_iter()
                        .map(|(k, d)| (k.to_owned(), d.to_owned()))
                        .collect(),
                )
            })
            .collect();

        // Nearest strict ancestor list: its `items` key names the item map
        // whose fields resolve first inside the template.
        let mut item_fields = Vec::new();
        for cut in (0..path.len()).rev() {
            let Some(anc) = doc_edit::node_at(&app.proj.document.root, &path[..cut]) else {
                continue;
            };
            if !matches!(anc.kind, NodeKind::List) {
                continue;
            }
            if let Some(key) = anc.bind.items.as_deref() {
                if let Some(sk) = info.state.get(key) {
                    item_fields =
                        sk.item.iter().map(|(f, ty)| (f.clone(), ty.clone())).collect();
                }
            }
            break;
        }
        Some(BindOptions { global, item_fields })
    }

    fn keys(&self, field: BindField) -> &[(String, String)] {
        self.global
            .iter()
            .find(|(f, _)| *f == field)
            .map(|(_, keys)| keys.as_slice())
            .unwrap_or(&[])
    }
}

#[derive(Default)]
struct Track {
    changed: bool,
    dragging: bool,
}

impl Track {
    fn hit(&mut self, r: Response) {
        self.changed |= r.changed();
        self.dragging |= r.dragged();
    }
}

pub fn show(app: &mut App, ui: &mut Ui) {
    ui.label(egui::RichText::new("Inspector").strong());
    let Some(path) = app.sel.clone() else {
        document_meta(app, ui);
        ui.separator();
        ui.label(egui::RichText::new("No node selected.").weak());
        return;
    };
    if path.is_empty() {
        document_meta(app, ui);
        ui.separator();
    }
    let Some(node) = doc_edit::node_at(&app.proj.document.root, &path) else {
        app.sel = None;
        return;
    };

    let mut edited = node.clone();
    let mut t = Track::default();
    let style_keys = app.theme.style_keys.clone();
    let bind_opts = BindOptions::build(app, &path);
    let mut focus_text = app.focus_text_edit;
    let is_root = path.is_empty();
    let project_dir = app.path.as_ref().and_then(|p| p.parent().map(std::path::PathBuf::from));
    let mut status_msg: Option<String> = None;

    ui.label(
        egui::RichText::new(format!("{}  {}", edited.kind.type_name(), path_str(&path)))
            .monospace()
            .weak(),
    );

    egui::CollapsingHeader::new("Node")
        .default_open(true)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("id");
                let mut id = edited.id.clone().unwrap_or_default();
                let r = ui.text_edit_singleline(&mut id);
                if r.changed() {
                    edited.id = (!id.is_empty()).then_some(id);
                }
                t.hit(r);
            });
            let mut ctx = KindCtx {
                style_keys: &style_keys,
                project_dir: project_dir.as_deref(),
                status: &mut status_msg,
            };
            kind_props(ui, &mut edited.kind, &mut t, &mut focus_text, &mut ctx);
        });

    egui::CollapsingHeader::new("Layout")
        .default_open(true)
        .show(ui, |ui| layout_props(ui, &mut edited, &mut t, is_root));

    egui::CollapsingHeader::new("Style")
        .default_open(true)
        .show(ui, |ui| {
            let current = edited.style.clone().unwrap_or_default();
            let shown = if current.is_empty() { "(widget default)" } else { &current };
            egui::ComboBox::from_id_salt("style_combo")
                .selected_text(shown)
                .width(180.0)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(current.is_empty(), "(widget default)").clicked() {
                        edited.style = None;
                        t.changed = true;
                    }
                    for key in &style_keys {
                        if ui.selectable_label(current == *key, key).clicked() {
                            edited.style = Some(key.clone());
                            t.changed = true;
                        }
                    }
                });
            ui.horizontal(|ui| {
                ui.label("custom");
                let mut s = edited.style.clone().unwrap_or_default();
                let r = ui.text_edit_singleline(&mut s);
                if r.changed() {
                    edited.style = (!s.is_empty()).then_some(s);
                }
                t.hit(r);
            });
        });

    egui::CollapsingHeader::new("Bindings")
        .default_open(false)
        .show(ui, |ui| {
            let rows: [(&str, BindField, &mut Option<String>); 7] = [
                ("text", BindField::Text, &mut edited.bind.text),
                ("value", BindField::Value, &mut edited.bind.value),
                ("enabled", BindField::Enabled, &mut edited.bind.enabled),
                ("visible", BindField::Visible, &mut edited.bind.visible),
                ("items", BindField::Items, &mut edited.bind.items),
                ("selected", BindField::Selected, &mut edited.bind.selected),
                ("image", BindField::Image, &mut edited.bind.image),
            ];
            for (label, field, v) in rows {
                match &bind_opts {
                    Some(opts) => bind_pick(ui, label, field, v, opts, &mut t),
                    None => opt_text(ui, label, v, &mut t),
                }
            }
        });

    app.focus_text_edit = focus_text;
    if let Some(msg) = status_msg {
        app.status = msg;
    }
    if t.changed {
        if t.dragging {
            app.history.begin_gesture(&app.proj.document);
        } else {
            app.history.record(&app.proj.document);
        }
        if let Some(slot) = doc_edit::node_at_mut(&mut app.proj.document.root, &path) {
            *slot = edited;
        }
        app.touch();
    }
}

fn path_str(path: &[usize]) -> String {
    let mut s = "root".to_owned();
    for i in path {
        s.push_str(&format!("/{i}"));
    }
    s
}

fn document_meta(app: &mut App, ui: &mut Ui) {
    ui.label(egui::RichText::new("Document").strong());
    let mut kind = app.proj.document.kind.clone();
    let mut class = app.proj.document.class;
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("kind");
        changed |= ui.text_edit_singleline(&mut kind).changed();
    });
    ui.horizontal(|ui| {
        ui.label("class");
        for (c, name) in [
            (DocClass::Screen, "screen"),
            (DocClass::Container, "container"),
            (DocClass::Hud, "hud"),
        ] {
            if ui.selectable_label(class == c, name).clicked() {
                class = c;
                changed = true;
            }
        }
    });
    if changed {
        app.mutate(|doc| {
            doc.kind = kind;
            doc.class = class;
        });
    }
}

fn opt_text(ui: &mut Ui, label: &str, v: &mut Option<String>, t: &mut Track) {
    ui.horizontal(|ui| {
        ui.label(label);
        let mut s = v.clone().unwrap_or_default();
        let r = ui.text_edit_singleline(&mut s);
        if r.changed() {
            *v = (!s.is_empty()).then_some(s);
        }
        t.hit(r);
    });
}

/// A binding field with a catalog picker: the combo offers the kind's
/// type-matching state keys (docs as tooltips) and — inside a list template —
/// the enclosing item's fields; the text box stays as the open-ended escape
/// hatch (mod keys).
fn bind_pick(
    ui: &mut Ui,
    label: &str,
    field: BindField,
    v: &mut Option<String>,
    opts: &BindOptions,
    t: &mut Track,
) {
    ui.horizontal(|ui| {
        ui.label(label);
        let current = v.clone().unwrap_or_default();
        let shown = if current.is_empty() { "(none)" } else { current.as_str() };
        egui::ComboBox::from_id_salt(("bind_pick", label))
            .selected_text(shown)
            .width(130.0)
            .show_ui(ui, |ui| {
                if ui.selectable_label(current.is_empty(), "(none)").clicked() {
                    *v = None;
                    t.changed = true;
                }
                for (key, doc) in opts.keys(field) {
                    let mut r = ui.selectable_label(current == *key, key);
                    if !doc.is_empty() {
                        r = r.on_hover_text(doc);
                    }
                    if r.clicked() {
                        *v = Some(key.clone());
                        t.changed = true;
                    }
                }
                let items: Vec<&(String, String)> = opts
                    .item_fields
                    .iter()
                    .filter(|(_, ty)| field_matches(field, ty))
                    .collect();
                if !items.is_empty() {
                    ui.separator();
                    ui.label(egui::RichText::new("list item fields").weak().small());
                    for (name, ty) in items {
                        let r = ui
                            .selectable_label(current == *name, name)
                            .on_hover_text(format!("item field ({ty})"));
                        if r.clicked() {
                            *v = Some(name.clone());
                            t.changed = true;
                        }
                    }
                }
            });
        let mut s = current.clone();
        let r = ui.add(egui::TextEdit::singleline(&mut s).desired_width(90.0));
        if r.changed() {
            *v = (!s.is_empty()).then_some(s);
        }
        t.hit(r);
    });
}

fn text_prop(ui: &mut Ui, label: &str, v: &mut Option<String>, t: &mut Track, focus: &mut bool) {
    ui.horizontal(|ui| {
        ui.label(label);
        let mut s = v.clone().unwrap_or_default();
        let r = ui.text_edit_singleline(&mut s);
        if *focus {
            r.request_focus();
            *focus = false;
        }
        if r.changed() {
            *v = (!s.is_empty()).then_some(s);
        }
        t.hit(r);
    });
}

struct KindCtx<'a> {
    style_keys: &'a [String],
    project_dir: Option<&'a std::path::Path>,
    /// Status-line message from a side-effecting action (image copy…).
    status: &'a mut Option<String>,
}

fn kind_props(ui: &mut Ui, kind: &mut NodeKind, t: &mut Track, focus: &mut bool, ctx: &mut KindCtx<'_>) {
    match kind {
        NodeKind::Label { text, wrap, scale } => {
            text_prop(ui, "text", text, t, focus);
            ui.horizontal(|ui| {
                let r = ui.checkbox(wrap, "wrap");
                t.hit(r);
                ui.label("scale");
                t.hit(ui.add(DragValue::new(scale).range(1..=4)));
            });
        }
        NodeKind::Button { text, icon } => {
            text_prop(ui, "text", text, t, focus);
            // Icon = a theme part key (e.g. `icon.edit`), drawn centred when
            // there's no label, else left of it.
            ui.horizontal(|ui| {
                ui.label("icon");
                let current = icon.clone().unwrap_or_default();
                let shown = if current.is_empty() { "(none)" } else { current.as_str() };
                egui::ComboBox::from_id_salt("button_icon")
                    .selected_text(shown)
                    .width(150.0)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(current.is_empty(), "(none)").clicked() {
                            *icon = None;
                            t.changed = true;
                        }
                        for key in ctx.style_keys {
                            if ui.selectable_label(current == *key, key).clicked() {
                                *icon = Some(key.clone());
                                t.changed = true;
                            }
                        }
                    });
            });
        }
        NodeKind::Toggle { icon } => {
            // Icon = a theme part key drawn centred on the toggle face
            // (on/off icon buttons like the craftable-only filter).
            ui.horizontal(|ui| {
                ui.label("icon");
                let current = icon.clone().unwrap_or_default();
                let shown = if current.is_empty() { "(none)" } else { current.as_str() };
                egui::ComboBox::from_id_salt("toggle_icon")
                    .selected_text(shown)
                    .width(150.0)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(current.is_empty(), "(none)").clicked() {
                            *icon = None;
                            t.changed = true;
                        }
                        for key in ctx.style_keys {
                            if ui.selectable_label(current == *key, key).clicked() {
                                *icon = Some(key.clone());
                                t.changed = true;
                            }
                        }
                    });
            });
        }
        NodeKind::Badge { text } => {
            text_prop(ui, "text", text, t, focus);
        }
        NodeKind::Alert { level, text } => {
            text_prop(ui, "text", text, t, focus);
            ui.horizontal(|ui| {
                ui.label("level");
                for (l, name) in [
                    (AlertLevel::Info, "info"),
                    (AlertLevel::Warning, "warning"),
                    (AlertLevel::Success, "success"),
                    (AlertLevel::Danger, "danger"),
                ] {
                    if ui.selectable_label(*level == l, name).clicked() {
                        *level = l;
                        t.changed = true;
                    }
                }
            });
        }
        NodeKind::Image { image, fit, interactive } => {
            string_prop(ui, "image", image, t);
            ui.horizontal(|ui| {
                if ui
                    .button("Choose image…")
                    .on_hover_text("Copies the PNG next to the project file")
                    .clicked()
                {
                    match crate::io::choose_project_image(ctx.project_dir) {
                        Ok(Some(name)) => {
                            *ctx.status = Some(format!("copied '{name}' beside the project"));
                            *image = name;
                            t.changed = true;
                        }
                        Ok(None) => {}
                        Err(e) => *ctx.status = Some(e),
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("fit");
                let is_slice = matches!(fit, ImageFit::Slice(_));
                for (mode, name) in [
                    (ImageFit::Stretch, "stretch"),
                    (ImageFit::Cover, "cover"),
                    (ImageFit::Tile, "tile"),
                ] {
                    if ui.selectable_label(*fit == mode, name).clicked() {
                        *fit = mode;
                        t.changed = true;
                    }
                }
                if ui.selectable_label(is_slice, "slice").clicked() && !is_slice {
                    *fit = ImageFit::Slice([4, 4, 4, 4]);
                    t.changed = true;
                }
            });
            if let ImageFit::Slice(insets) = fit {
                quad_edit(ui, "insets", insets, t);
            }
            t.hit(ui.checkbox(interactive, "interactive canvas"));
        }
        NodeKind::Rotimage { image, pivot } => {
            string_prop(ui, "image", image, t);
            ui.horizontal(|ui| {
                let mut has = pivot.is_some();
                let r = ui.checkbox(&mut has, "pivot");
                if r.changed() {
                    *pivot = has.then_some([0.0, 0.0]);
                }
                t.hit(r);
                if let Some(p) = pivot {
                    t.hit(ui.add(DragValue::new(&mut p[0]).speed(0.5)));
                    t.hit(ui.add(DragValue::new(&mut p[1]).speed(0.5)));
                }
            });
        }
        NodeKind::Slider { min, max, step } => {
            ui.horizontal(|ui| {
                ui.label("min");
                t.hit(ui.add(DragValue::new(min).speed(1.0)));
                ui.label("max");
                t.hit(ui.add(DragValue::new(max).speed(1.0)));
            });
            ui.horizontal(|ui| {
                let mut has = step.is_some();
                let r = ui.checkbox(&mut has, "step");
                if r.changed() {
                    *step = has.then_some(1.0);
                }
                t.hit(r);
                if let Some(s) = step {
                    t.hit(ui.add(DragValue::new(s).speed(0.25)));
                }
            });
        }
        NodeKind::TextInput { placeholder, max_chars } => {
            text_prop(ui, "placeholder", placeholder, t, focus);
            ui.horizontal(|ui| {
                ui.label("max chars");
                t.hit(ui.add(DragValue::new(max_chars).range(1..=1024)));
            });
        }
        NodeKind::Scroll { axis } => {
            ui.horizontal(|ui| {
                ui.label("axis");
                for (a, name) in [(ScrollAxis::Vertical, "vertical"), (ScrollAxis::Horizontal, "horizontal")] {
                    if ui.selectable_label(*axis == a, name).clicked() {
                        *axis = a;
                        t.changed = true;
                    }
                }
            });
        }
        NodeKind::Slot { role, .. } => {
            string_prop(ui, "role", role, t);
        }
        NodeKind::SlotGrid { role, cols, rows, .. } => {
            string_prop(ui, "role", role, t);
            ui.horizontal(|ui| {
                ui.label("cols");
                t.hit(ui.add(DragValue::new(cols).range(1..=32)));
                ui.label("rows");
                t.hit(ui.add(DragValue::new(rows).range(1..=32)));
            });
        }
        NodeKind::Gauge { mode } => {
            ui.horizontal(|ui| {
                ui.label("mode");
                for (m, name) in [(GaugeMode::GrowLr, "grow_lr"), (GaugeMode::DepleteTd, "deplete_td")] {
                    if ui.selectable_label(*mode == m, name).clicked() {
                        *mode = m;
                        t.changed = true;
                    }
                }
            });
        }
        NodeKind::Frame
        | NodeKind::Row
        | NodeKind::Column
        | NodeKind::Spacer
        | NodeKind::Checkbox
        | NodeKind::List
        | NodeKind::Hook => {}
    }
}

fn string_prop(ui: &mut Ui, label: &str, v: &mut String, t: &mut Track) {
    ui.horizontal(|ui| {
        ui.label(label);
        t.hit(ui.text_edit_singleline(v));
    });
}

fn layout_props(ui: &mut Ui, node: &mut petramond_ui::Node, t: &mut Track, is_root: bool) {
    let l = &mut node.layout;
    size_edit(ui, "w", &mut l.w, t);
    size_edit(ui, "h", &mut l.h, t);
    quad_edit(ui, "pad", &mut l.pad, t);
    quad_edit(ui, "margin", &mut l.margin, t);
    ui.horizontal(|ui| {
        ui.label("gap");
        t.hit(ui.add(DragValue::new(&mut l.gap)));
    });
    if matches!(
        node.kind,
        NodeKind::Frame | NodeKind::Button { .. } | NodeKind::Scroll { .. }
    ) {
        ui.horizontal(|ui| {
            ui.label("dir");
            for (d, name) in [(Dir::Row, "row"), (Dir::Column, "column")] {
                if ui.selectable_label(l.dir == d, name).clicked() {
                    l.dir = d;
                    t.changed = true;
                }
            }
        });
    }
    ui.horizontal(|ui| {
        ui.label("align");
        // `None` = the node type's default (scroll stretches, others start).
        if ui
            .selectable_label(l.align.is_none(), "auto")
            .on_hover_text("node default: scroll stretches its children, others start")
            .clicked()
        {
            l.align = None;
            t.changed = true;
        }
        for (a, name) in [
            (Align::Start, "start"),
            (Align::Center, "center"),
            (Align::End, "end"),
            (Align::Stretch, "stretch"),
        ] {
            if ui.selectable_label(l.align == Some(a), name).clicked() {
                l.align = Some(a);
                t.changed = true;
            }
        }
    });
    ui.horizontal(|ui| {
        ui.label("justify");
        for (j, name) in [
            (Justify::Start, "start"),
            (Justify::Center, "center"),
            (Justify::End, "end"),
            (Justify::SpaceBetween, "between"),
        ] {
            if ui.selectable_label(l.justify == j, name).clicked() {
                l.justify = j;
                t.changed = true;
            }
        }
    });
    ui.horizontal(|ui| {
        opt_i32(ui, "min w", &mut l.min_w, t);
        opt_i32(ui, "min h", &mut l.min_h, t);
    });
    ui.horizontal(|ui| {
        opt_i32(ui, "max w", &mut l.max_w, t);
        opt_i32(ui, "max h", &mut l.max_h, t);
    });
    ui.horizontal(|ui| {
        let mut has = l.abs.is_some();
        let r = ui.checkbox(&mut has, "abs");
        if r.changed() {
            l.abs = has.then_some(AbsPos { x: 0, y: 0 });
        }
        t.hit(r);
        if let Some(abs) = &mut l.abs {
            ui.label("x");
            t.hit(ui.add(DragValue::new(&mut abs.x)));
            ui.label("y");
            t.hit(ui.add(DragValue::new(&mut abs.y)));
        }
    });
    if is_root {
        ui.horizontal(|ui| {
            let mut has = l.anchor.is_some();
            let r = ui.checkbox(&mut has, "anchor");
            if r.changed() {
                l.anchor = has.then_some(Anchor::default());
            }
            t.hit(r);
        });
        if let Some(anchor) = &mut l.anchor {
            for (edge, label) in [(&mut anchor.h, "h"), (&mut anchor.v, "v")] {
                ui.horizontal(|ui| {
                    ui.label(label);
                    for (e, name) in [
                        (AnchorEdge::Start, "start"),
                        (AnchorEdge::Center, "center"),
                        (AnchorEdge::End, "end"),
                    ] {
                        if ui.selectable_label(*edge == e, name).clicked() {
                            *edge = e;
                            t.changed = true;
                        }
                    }
                });
            }
        }
    }
}

fn size_edit(ui: &mut Ui, label: &str, s: &mut Size, t: &mut Track) {
    ui.horizontal(|ui| {
        ui.label(label);
        let (is_auto, is_px, is_grow) = (
            matches!(s, Size::Auto),
            matches!(s, Size::Px(_)),
            matches!(s, Size::Grow(_)),
        );
        if ui.selectable_label(is_auto, "auto").clicked() && !is_auto {
            *s = Size::Auto;
            t.changed = true;
        }
        if ui.selectable_label(is_px, "px").clicked() && !is_px {
            *s = Size::Px(20);
            t.changed = true;
        }
        if ui.selectable_label(is_grow, "grow").clicked() && !is_grow {
            *s = Size::Grow(1);
            t.changed = true;
        }
        match s {
            Size::Px(v) => t.hit(ui.add(DragValue::new(v).range(0..=4096))),
            Size::Grow(g) => t.hit(ui.add(DragValue::new(g).range(1..=100))),
            Size::Auto => {}
        }
    });
}

fn quad_edit(ui: &mut Ui, label: &str, v: &mut [i32; 4], t: &mut Track) {
    ui.horizontal(|ui| {
        ui.label(label);
        for x in v.iter_mut() {
            t.hit(ui.add(DragValue::new(x).range(-512..=512)));
        }
    });
}

fn opt_i32(ui: &mut Ui, label: &str, v: &mut Option<i32>, t: &mut Track) {
    let mut has = v.is_some();
    let r = ui.checkbox(&mut has, label);
    if r.changed() {
        *v = has.then_some(0);
    }
    t.hit(r);
    if let Some(x) = v {
        t.hit(ui.add(DragValue::new(x).range(0..=4096)));
    }
}
