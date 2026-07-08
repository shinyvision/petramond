//! The GUI theme loader: `assets/ui/theme/theme.json` + its images, through
//! the asset layering (a pack may ship a full replacement theme; the
//! highest-priority copy wins whole-file).
//!
//! Until the shipped kit exists (or when it fails to parse) the synthesized
//! placeholder theme renders instead — a GUI with programmer-art chrome beats
//! a panic or a blank screen, and the loud magenta missing-part color makes
//! gaps obvious.

use petramond_ui::Theme;
use std::sync::{Arc, OnceLock};

const THEME_JSON: &str = "ui/theme/theme.json";

static THEME: OnceLock<Arc<Theme>> = OnceLock::new();

pub(crate) fn theme() -> Arc<Theme> {
    THEME.get_or_init(load).clone()
}

fn load() -> Arc<Theme> {
    // Highest-priority copy wins: read_layers returns base first, packs after.
    let Some((json, path)) = crate::assets::read_layers(THEME_JSON)
        .into_iter()
        .next_back()
    else {
        return Arc::new(Theme::placeholder());
    };
    let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    match Theme::load(&json, &|rel| std::fs::read(dir.join(rel)).ok()) {
        Ok(theme) => Arc::new(theme),
        Err(e) => {
            eprintln!(
                "gui: theme {} failed to load — {e}; using placeholder",
                path.display()
            );
            Arc::new(Theme::placeholder())
        }
    }
}
