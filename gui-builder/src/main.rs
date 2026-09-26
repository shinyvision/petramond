//! Petramond GUI Builder — a document editor for petramond-ui `*.gui.json` GUIs.
//! The preview embeds the real petramond-ui runtime through its software
//! rasterizer, so what you see is pixel-exactly what the game renders.
//!
//! CLI (the asset options go before the command):
//!   gui-builder [--assets <dir>] [--pack <dir>]... [project.llgui]
//!   gui-builder [...] --export <in.llgui> [out.gui.json]
//!   gui-builder [...] --screenshot <project.llgui> <out.png>
//!   gui-builder [...] --make-samples

mod app;
mod assets;
mod bindings;
mod canvas;
mod doc_edit;
mod engine_check;
mod history;
mod io;
mod panels;
mod preview;
mod project;
mod theme_bar;
mod theme_src;

use assets::AssetRoots;
use std::path::{Path, PathBuf};

const USAGE: &str = "\
gui-builder [--assets <dir>] [--pack <dir>]... [project.llgui]
gui-builder [...] --export <in.llgui> [out.gui.json]
gui-builder [...] --screenshot <project.llgui> <out.png>
gui-builder [...] --make-samples   (regenerate samples/ from shipped documents)

--assets <dir>  the base game assets (default: the nearest assets/ above the
                working directory or the executable)
--pack <dir>    a pack root layered over the base, highest priority last
                (repeatable); a project saved inside a pack layers it too";

fn main() {
    let result = parse(std::env::args().skip(1).collect()).and_then(|(roots, args)| {
        match args.first().map(String::as_str) {
            Some("--export") => cli_export(&roots, &args[1..]),
            Some("--screenshot") => cli_screenshot(&roots, &args[1..]),
            Some("--make-samples") => io::make_samples(&roots).map(|made| {
                println!(
                    "regenerated {} samples: {}",
                    made.regenerated.len(),
                    made.regenerated.join(", ")
                );
                if !made.deleted.is_empty() {
                    println!(
                        "deleted {} orphaned samples: {}",
                        made.deleted.len(),
                        made.deleted.join(", ")
                    );
                }
            }),
            Some("--help" | "-h") => {
                println!("{USAGE}");
                Ok(())
            }
            first => run_gui(first.map(PathBuf::from), roots).map_err(|e| e.to_string()),
        }
    });
    if let Err(e) = result {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

/// Split the leading asset options off the command line.
fn parse(args: Vec<String>) -> Result<(AssetRoots, Vec<String>), String> {
    let mut base = None;
    let mut packs = Vec::new();
    let mut rest = args.into_iter();
    let mut remaining = Vec::new();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--assets" | "--pack" => {
                let dir = rest
                    .next()
                    .map(PathBuf::from)
                    .ok_or_else(|| format!("{arg} needs a directory\n\n{USAGE}"))?;
                if !dir.is_dir() {
                    return Err(format!("{arg} {}: not a directory", dir.display()));
                }
                if arg == "--assets" {
                    base = Some(dir);
                } else {
                    packs.push(dir);
                }
            }
            _ => {
                remaining.push(arg);
                remaining.extend(rest);
                break;
            }
        }
    }
    Ok((AssetRoots::new(base, packs), remaining))
}

fn run_gui(open: Option<PathBuf>, roots: AssetRoots) -> Result<(), eframe::Error> {
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
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, open, roots)) as Box<dyn eframe::App>)),
    )
}

/// Headless export: write the project's bare document as `.gui.json`.
fn cli_export(roots: &AssetRoots, args: &[String]) -> Result<(), String> {
    let input = args.first().ok_or("--export needs an input .llgui path")?;
    let input = Path::new(input);
    let project = io::load_project(input)?;
    let out = match args.get(1) {
        Some(p) => PathBuf::from(p),
        None => input.with_file_name(io::export_file_name(&project.document)),
    };
    // The game's own load-time rules: anything reported here is a document
    // the game would skip.
    let dir = input.parent();
    let roots = roots.for_project(dir, &project.editor.asset_roots);
    let theme = theme_src::load(&roots, 0);
    let (state, _) = project.sample_ui_state();
    let ctx = engine_check::EngineContext::for_project(&roots, dir);
    for issue in ctx.validate(&project.document, &theme.theme, &state, &|name| {
        io::resolve_document_image_path(&roots, dir, name)
    }) {
        eprintln!("warning: {issue}");
    }
    let copied = io::export_document(&out, &project.document, input.parent())?;
    println!("exported {} (+{copied} images)", out.display());
    Ok(())
}

/// Render a project's preview (no editor chrome) to a PNG — the end-to-end
/// verification path for the preview pipeline.
fn cli_screenshot(roots: &AssetRoots, args: &[String]) -> Result<(), String> {
    let (input, output) = match args {
        [i, o, ..] => (Path::new(i), Path::new(o)),
        _ => return Err("--screenshot needs <project.llgui> <out.png>".into()),
    };
    let project = io::load_project(input)?;
    let doc_dir = input.parent();
    let roots = roots.for_project(doc_dir, &project.editor.asset_roots);
    let theme = theme_src::load(&roots, 0);
    let catalog = bindings::Catalog::load(&roots);
    let (rgba, (w, h)) =
        preview::render_project(&project, &theme.theme, &roots, doc_dir, catalog.as_ref());
    image::save_buffer(output, &rgba, w, h, image::ColorType::Rgba8)
        .map_err(|e| format!("write {}: {e}", output.display()))?;
    println!(
        "wrote {} ({}x{}, theme: {})",
        output.display(),
        w,
        h,
        theme.label
    );
    Ok(())
}
