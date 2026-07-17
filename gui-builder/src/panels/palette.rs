//! Component palette: one button per node type plus document-fragment presets
//! (compiled in from `assets/presets/*.json`). Clicking inserts into the
//! selected container (ids are uniquified on insert).

use crate::app::App;
use crate::doc_edit;
use eframe::egui;
use petramond_ui::Node;

const PRESETS: &[(&str, &str)] = &[
    ("Titled section", include_str!("../../assets/presets/titled_section.json")),
    ("Labeled slider row", include_str!("../../assets/presets/labeled_slider.json")),
    ("Mod list row template", include_str!("../../assets/presets/mod_list.json")),
    ("Alert", include_str!("../../assets/presets/alert.json")),
    ("Tab bar with pages", include_str!("../../assets/presets/tab_bar.json")),
];

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    ui.label(egui::RichText::new("Palette").strong());
    ui.label(egui::RichText::new("Inserts into the selected container.").weak().small());
    ui.horizontal_wrapped(|ui| {
        let mut insert: Option<Node> = None;
        for ty in doc_edit::NODE_TYPES {
            if ui.button(*ty).clicked() {
                insert = doc_edit::new_node(&app.proj.document, ty);
            }
        }
        if let Some(node) = insert {
            app.insert_node(node);
        }
    });
    ui.separator();
    ui.label(egui::RichText::new("Presets").strong());
    for (name, json) in PRESETS {
        if ui.button(*name).clicked() {
            match serde_json::from_str::<Node>(json) {
                Ok(node) => app.insert_node(node),
                Err(e) => app.status = format!("preset '{name}': {e}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_parses_as_a_node_fragment() {
        for (name, json) in PRESETS {
            let node: Node = serde_json::from_str(json).unwrap_or_else(|e| panic!("{name}: {e}"));
            // Fragments must serialize back (they're inserted verbatim).
            serde_json::to_string(&node).unwrap();
        }
    }
}
