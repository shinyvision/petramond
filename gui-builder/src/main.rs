//! Llamacraft GUI Builder — a standalone tool for composing data-driven GUIs
//! from reusable parts, placing typed item slots, and baking a PNG + JSON
//! manifest the game can render.

mod app;
mod assets;
mod bake;
mod canvas;
mod icons;
mod model;
mod ui;

fn main() -> Result<(), eframe::Error> {
    let native_options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1360.0, 860.0])
            .with_min_inner_size([900.0, 600.0])
            .with_title("Llamacraft GUI Builder"),
        ..Default::default()
    };
    eframe::run_native(
        "Llamacraft GUI Builder",
        native_options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)) as Box<dyn eframe::App>)),
    )
}
