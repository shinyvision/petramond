//! The preview toolbar: theme source, gui scale, screen-size presets, forced
//! widget states for the selection, zoom, and the sample-state editor toggle.

use crate::app::App;
use crate::theme_src;
use eframe::egui;

const SCREEN_PRESETS: [(u32, u32); 3] = [(1280, 720), (1920, 1080), (854, 480)];

pub fn show(app: &mut App, ctx: &egui::Context) {
    egui::TopBottomPanel::top("theme_bar").show(ctx, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new(&app.theme.label).weak());
            if ui.button("⟳").on_hover_text("Reload theme").clicked() {
                app.theme = theme_src::load(app.theme.rev + 1);
                app.touch();
            }
            ui.separator();

            ui.label("scale");
            let mut scale = app.proj.editor.preview_scale.clamp(1, 4);
            for s in 1..=4u32 {
                if ui.selectable_label(scale == s, format!("{s}x")).clicked() {
                    scale = s;
                }
            }
            if scale != app.proj.editor.preview_scale {
                app.proj.editor.preview_scale = scale;
                app.touch();
            }
            ui.separator();

            ui.label("screen");
            let cur = app.proj.editor.screen;
            egui::ComboBox::from_id_salt("screen_preset")
                .selected_text(format!("{}x{}", cur.0, cur.1))
                .show_ui(ui, |ui| {
                    for (w, h) in SCREEN_PRESETS {
                        if ui
                            .selectable_label(cur == (w, h), format!("{w}x{h}"))
                            .clicked()
                        {
                            app.proj.editor.screen = (w, h);
                            app.touch();
                        }
                    }
                });
            ui.separator();

            ui.label("zoom");
            let mut zoom = app.proj.editor.zoom;
            if ui
                .add(egui::Slider::new(&mut zoom, 0.25..=8.0).logarithmic(true))
                .changed()
            {
                app.proj.editor.zoom = zoom;
            }
            ui.separator();

            // Forced states preview the selection's hover/pressed/focus faces
            // without real input (needs an id-bearing node).
            let has_id = app.selected_node().is_some_and(|n| n.id.is_some());
            ui.add_enabled_ui(has_id, |ui| {
                ui.label("force:");
                let mut f = app.forced.clone();
                ui.checkbox(&mut f.hover, "hover");
                ui.checkbox(&mut f.pressed, "pressed");
                ui.checkbox(&mut f.focus, "focus");
                if f != app.forced {
                    app.forced = f;
                }
            });
            if !has_id {
                ui.label(egui::RichText::new("(select an id-bearing node)").weak().small());
            }
            ui.separator();

            if ui
                .selectable_label(app.sample_open, "sample state…")
                .clicked()
            {
                app.sample_open = !app.sample_open;
            }
            ui.separator();
            ui.label(egui::RichText::new(&app.status).weak());
        });
    });
}
