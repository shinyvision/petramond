//! Live validation panel: `Document::validate` against the loaded theme and
//! the kind's engine contract. Clicking an issue selects the offending node.

use crate::app::App;
use crate::doc_edit;
use eframe::egui;

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let issues = app.validation();
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Validation").strong());
        if issues.is_empty() {
            ui.colored_label(egui::Color32::from_rgb(110, 200, 120), "✔ document is valid");
        } else {
            ui.colored_label(
                egui::Color32::from_rgb(235, 160, 80),
                format!("{} issue(s)", issues.len()),
            );
        }
    });
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (i, issue) in issues.iter().enumerate() {
                let text = format!("{}: {}", issue.path, issue.message);
                if ui
                    .selectable_label(false, egui::RichText::new(text).monospace().small())
                    .on_hover_text("Click to select the offending node")
                    .clicked()
                {
                    if let Some(path) = doc_edit::resolve_issue_path(&issue.path) {
                        if doc_edit::node_at(&app.proj.document.root, &path).is_some() {
                            app.select_external(Some(path));
                        }
                    }
                }
                let _ = i;
            }
        });
}
