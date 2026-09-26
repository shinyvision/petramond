//! The GUI theme loader: `assets/ui/theme/theme.json` + its images, through
//! the asset layering. Every layer's copy joins the theme STACK (base first,
//! packs above it): a pack adds or replaces parts by key on its own atlas
//! page, adds palette entries and metrics, and may bring a font — so several
//! packs can each add chrome without replacing the kit (see
//! [`petramond_ui::Theme::load_stack`]).
//!
//! Until the shipped kit exists (or when it fails to parse) the synthesized
//! placeholder theme renders instead — a GUI with programmer-art chrome beats
//! a panic or a blank screen, and the loud magenta missing-part color makes
//! gaps obvious. A broken pack layer is reported and the stack loads without
//! the pack layers rather than losing the base kit.

use petramond_ui::{Theme, ThemeLayer};
use std::sync::{Arc, OnceLock};

const THEME_JSON: &str = "ui/theme/theme.json";

static THEME: OnceLock<Arc<Theme>> = OnceLock::new();

pub fn theme() -> Arc<Theme> {
    THEME.get_or_init(load).clone()
}

/// The theme's UI font: what text surfaces outside documents (chat, mod
/// canvases) measure and paint with, so they match document text. There is
/// no process-global font — every measurement names the font it uses.
pub fn ui_font() -> Arc<petramond_ui::text::Font> {
    theme().ui_font().clone()
}

fn load() -> Arc<Theme> {
    // read_layers returns base first, packs after: exactly stack order.
    let layers = petramond_world::assets::read_layers(THEME_JSON);
    if layers.is_empty() {
        return Arc::new(Theme::placeholder());
    }
    let readers: Vec<_> = layers
        .iter()
        .map(|(_, path)| {
            let dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
            move |rel: &str| std::fs::read(dir.join(rel)).ok()
        })
        .collect();
    let stack: Vec<ThemeLayer<'_>> = layers
        .iter()
        .zip(&readers)
        .map(|((json, _), read)| ThemeLayer { json, read })
        .collect();
    let describe = |n: usize| {
        layers[..n]
            .iter()
            .map(|(_, path)| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    match Theme::load_stack(&stack) {
        Ok(theme) => return Arc::new(theme),
        Err(e) => eprintln!("gui: theme stack [{}] failed to load — {e}", describe(stack.len())),
    }
    if stack.len() > 1 {
        match Theme::load_stack(&stack[..1]) {
            Ok(theme) => {
                eprintln!("gui: using the base theme without pack layers");
                return Arc::new(theme);
            }
            Err(e) => eprintln!("gui: base theme {} failed to load — {e}", describe(1)),
        }
    }
    eprintln!("gui: using the placeholder theme");
    Arc::new(Theme::placeholder())
}
