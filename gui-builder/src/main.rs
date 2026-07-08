//! Petramond GUI Builder — a document editor for petramond-ui `*.gui.json` GUIs.
//! The preview embeds the real petramond-ui runtime through its software
//! rasterizer, so what you see is pixel-exactly what the game renders.
//!
//! CLI:
//!   gui-builder [project.llgui]                 open the editor
//!   gui-builder --export <in.llgui> [out.gui.json]
//!   gui-builder --import-legacy <v1.llgui> <out.llgui>
//!   gui-builder --screenshot <project.llgui> <out.png>

mod app;
mod bindings;
mod canvas;
mod contracts;
mod doc_edit;
mod history;
mod io;
mod legacy_import;
mod panels;
mod preview;
mod project;
mod theme_bar;
mod theme_src;

use std::path::{Path, PathBuf};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("--export") => cli_export(&args[1..]),
        Some("--import-legacy") => cli_import_legacy(&args[1..]),
        Some("--screenshot") => cli_screenshot(&args[1..]),
        Some("--make-samples") => match io::make_samples() {
            Ok(names) => {
                println!("regenerated {} samples: {}", names.len(), names.join(", "));
                Ok(())
            }
            Err(e) => Err(e),
        },
        Some("--help" | "-h") => {
            println!(
                "gui-builder [project.llgui]\n\
                 gui-builder --export <in.llgui> [out.gui.json]\n\
                 gui-builder --import-legacy <v1.llgui> <out.llgui>\n\
                 gui-builder --screenshot <project.llgui> <out.png>\n\
                 gui-builder --make-samples   (regenerate samples/ from shipped documents)"
            );
            Ok(())
        }
        first => run_gui(first.map(PathBuf::from)).map_err(|e| e.to_string()),
    };
    if let Err(e) = result {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn run_gui(open: Option<PathBuf>) -> Result<(), eframe::Error> {
    let native_options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([980.0, 620.0])
            .with_title("Petramond GUI Builder"),
        ..Default::default()
    };
    eframe::run_native(
        "Petramond GUI Builder",
        native_options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, open)) as Box<dyn eframe::App>)),
    )
}

/// Headless export: write the project's bare document as `.gui.json`.
fn cli_export(args: &[String]) -> Result<(), String> {
    let input = args.first().ok_or("--export needs an input .llgui path")?;
    let input = Path::new(input);
    let project = io::load_project(input)?;
    let out = match args.get(1) {
        Some(p) => PathBuf::from(p),
        None => input.with_file_name(io::export_file_name(&project.document)),
    };
    let contract = contracts::contract_for(&project.document.kind);
    for issue in project.document.validate(None, Some(&contract)) {
        eprintln!("warning: {issue}");
    }
    let copied = io::export_document(&out, &project.document, input.parent())?;
    println!("exported {} (+{copied} images)", out.display());
    Ok(())
}

/// Convert a legacy v1 `.llgui` into a v2 project.
fn cli_import_legacy(args: &[String]) -> Result<(), String> {
    let (input, output) = match args {
        [i, o, ..] => (Path::new(i), Path::new(o)),
        _ => return Err("--import-legacy needs <v1.llgui> <out.llgui>".into()),
    };
    let (project, warnings) = io::import_legacy(input)?;
    for w in &warnings {
        eprintln!("warning: {w}");
    }
    io::save_project(output, &project)?;
    println!(
        "imported {} -> {} ({} warnings)",
        input.display(),
        output.display(),
        warnings.len()
    );
    Ok(())
}

/// Render a project's preview (no editor chrome) to a PNG — the end-to-end
/// verification path for the preview pipeline.
fn cli_screenshot(args: &[String]) -> Result<(), String> {
    let (input, output) = match args {
        [i, o, ..] => (Path::new(i), Path::new(o)),
        _ => return Err("--screenshot needs <project.llgui> <out.png>".into()),
    };
    let project = io::load_project(input)?;
    let theme = theme_src::load(0);
    let catalog = bindings::Catalog::load();
    let doc_dir = input.parent();
    let (rgba, (w, h)) =
        preview::render_project(&project, &theme.theme, doc_dir, catalog.as_ref());
    image::save_buffer(output, &rgba, w, h, image::ColorType::Rgba8)
        .map_err(|e| format!("write {}: {e}", output.display()))?;
    println!("wrote {} ({}x{}, theme: {})", output.display(), w, h, theme.label);
    Ok(())
}
