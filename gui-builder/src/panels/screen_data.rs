//! Screen-data panel: the current kind's full data catalog from
//! `assets/ui/bindings.json` — every state key the game populates (bindable)
//! and every widget id it reacts to — so an author can read what a screen can
//! do without leaving the tool.

use crate::app::App;
use eframe::egui;

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let force_open = app.screen_data_force_open.take();
    let kind = app.proj.document.kind.clone();
    let Some(catalog) = &app.catalog else { return };

    egui::CollapsingHeader::new(egui::RichText::new("Screen data").strong())
        .default_open(true)
        .open(force_open)
        .show(ui, |ui| {
            let Some(info) = catalog.kind(&kind) else {
                ui.label(
                    egui::RichText::new(format!(
                        "No catalog entry for '{kind}'. Mod kinds bind whatever keys the mod \
                         sets via its GUI state map; button ids are dispatched to the mod as \
                         gui_click."
                    ))
                    .weak(),
                );
                return;
            };
            egui::ScrollArea::vertical()
                .id_salt("screen_data_scroll")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if !info.state.is_empty() {
                        ui.label(egui::RichText::new("State keys (bind these)").strong().small());
                        for (name, key) in &info.state {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(
                                    egui::RichText::new(format!("{name}: {}", key.ty)).monospace(),
                                );
                                if !key.item.is_empty() {
                                    let fields = key
                                        .item
                                        .iter()
                                        .map(|(f, ty)| format!("{f}: {ty}"))
                                        .collect::<Vec<_>>()
                                        .join(", ");
                                    ui.label(
                                        egui::RichText::new(format!("{{ {fields} }}"))
                                            .monospace()
                                            .weak(),
                                    );
                                }
                                if !key.doc.is_empty() {
                                    ui.label(egui::RichText::new(format!("— {}", key.doc)).weak());
                                }
                            });
                        }
                    }
                    if !info.handles.is_empty() {
                        ui.label(
                            egui::RichText::new("Handles (widget ids the game reacts to)")
                                .strong()
                                .small(),
                        );
                        for (id, desc) in &info.handles {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(egui::RichText::new(format!("#{id}")).monospace());
                                ui.label(egui::RichText::new(format!("— {desc}")).weak());
                            });
                        }
                    }
                    if info.state.is_empty() && info.handles.is_empty() {
                        ui.label(
                            egui::RichText::new("This kind binds no dynamic data and reacts to no widget ids.")
                                .weak(),
                        );
                    }
                    if let Some(notes) = &info.notes {
                        ui.label(egui::RichText::new(notes).weak().italics());
                    }
                });
        });
}
