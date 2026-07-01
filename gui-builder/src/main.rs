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

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.iter().any(|arg| arg == "--bake-shell-defaults") {
        if let Err(e) = bake_shell_defaults() {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }
    if let Some(i) = args.iter().position(|arg| arg == "--bake-shell-default") {
        let Some(stem) = args.get(i + 1) else {
            eprintln!(
                "--bake-shell-default needs one of: title, world_select, create_world, delete_world, pause"
            );
            std::process::exit(1);
        };
        if let Err(e) = bake_shell_default(stem) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }
    if let Some(i) = args.iter().position(|arg| arg == "--bake-project") {
        let Some(input) = args.get(i + 1) else {
            eprintln!("--bake-project needs an input .llgui path");
            std::process::exit(1);
        };
        let output = args.get(i + 2).map(String::as_str);
        if let Err(e) = bake_project(input, output) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }

    if let Err(e) = run_gui() {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn run_gui() -> Result<(), eframe::Error> {
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

fn bake_shell_defaults() -> Result<(), String> {
    for (_, stem) in shell_defaults() {
        bake_shell_default(stem)?;
    }
    Ok(())
}

fn bake_shell_default(stem: &str) -> Result<(), String> {
    let Some((ty, stem)) = shell_defaults()
        .into_iter()
        .find(|(_, candidate)| *candidate == stem)
    else {
        return Err(format!(
            "unknown shell default '{stem}' (expected title, world_select, create_world, delete_world, or pause)"
        ));
    };
    let builder_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = builder_dir
        .parent()
        .ok_or_else(|| "gui-builder has no parent directory".to_string())?;
    let guis_dir = root.join("guis");
    let baked_dir = root.join("assets/textures/gui/shell/baked");
    std::fs::create_dir_all(&guis_dir).map_err(|e| format!("create guis dir: {e}"))?;
    std::fs::create_dir_all(&baked_dir).map_err(|e| format!("create shell baked dir: {e}"))?;

    let ctx = eframe::egui::Context::default();
    let assets = assets::AssetLibrary::new(&ctx);
    let project = model::Project::new(ty);
    let project_json =
        serde_json::to_string_pretty(&project).map_err(|e| format!("encode {stem}: {e}"))?;
    std::fs::write(guis_dir.join(format!("{stem}.llgui")), project_json)
        .map_err(|e| format!("write {stem}.llgui: {e}"))?;
    bake::bake_to_files(&project, &assets, &baked_dir.join(format!("{stem}.png")))?;

    Ok(())
}

fn bake_project(input: &str, output: Option<&str>) -> Result<(), String> {
    let input = std::path::PathBuf::from(input);
    let text =
        std::fs::read_to_string(&input).map_err(|e| format!("read {}: {e}", input.display()))?;
    let project: model::Project =
        serde_json::from_str(&text).map_err(|e| format!("decode {}: {e}", input.display()))?;

    let ctx = eframe::egui::Context::default();
    let mut assets = assets::AssetLibrary::new(&ctx);
    ensure_project_assets(&ctx, &mut assets, &project)?;

    let png_path = output
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| input.with_extension("png"));
    bake::bake_to_files(&project, &assets, &png_path)
}

fn ensure_project_assets(
    ctx: &eframe::egui::Context,
    assets: &mut assets::AssetLibrary,
    project: &model::Project,
) -> Result<(), String> {
    let mut missing = Vec::new();
    for fl in project.flat_layers() {
        if let Err(e) = assets.ensure(ctx, &fl.layer.asset) {
            missing.push(e);
        }
    }
    if let Some(hover) = &project.hover {
        if let Err(e) = assets.ensure(ctx, &hover.asset) {
            missing.push(e);
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!("missing project asset(s): {}", missing.join("; ")))
    }
}

fn shell_defaults() -> [(model::GuiType, &'static str); 5] {
    [
        (model::GuiType::Title, "title"),
        (model::GuiType::WorldSelect, "world_select"),
        (model::GuiType::CreateWorld, "create_world"),
        (model::GuiType::DeleteWorld, "delete_world"),
        (model::GuiType::Pause, "pause"),
    ]
}
