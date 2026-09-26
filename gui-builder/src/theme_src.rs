//! Theme loading for the builder: the game's theme kit resolved through the
//! asset layers exactly like the game resolves it (the highest-priority
//! `ui/theme/theme.json` wins whole-file, so a pack's replacement theme
//! previews as it will ship), otherwise petramond-ui's placeholder.

use crate::assets::AssetRoots;
use petramond_ui::Theme;
use std::path::PathBuf;
use std::sync::Arc;

pub struct ThemeSource {
    pub theme: Arc<Theme>,
    /// Every part key the theme defines (feeds the inspector's style combo),
    /// straight from `Theme::style_keys()`.
    pub style_keys: Vec<String>,
    /// Human-readable origin for the toolbar ("game theme" / "placeholder").
    pub label: String,
    /// Bumped on every (re)load so the preview cache invalidates.
    pub rev: u64,
}

const THEME_JSON: &str = "ui/theme/theme.json";

pub fn load(roots: &AssetRoots, rev: u64) -> ThemeSource {
    let Some(path) = roots.find(THEME_JSON) else {
        return source(Theme::placeholder(), "placeholder theme".into(), rev);
    };
    let loaded = std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|json| {
            let dir = path.parent().map(PathBuf::from).unwrap_or_default();
            let read = |name: &str| std::fs::read(dir.join(name)).ok();
            Theme::load(&json, &read).map_err(|e| e.to_string())
        });
    match loaded {
        Ok(theme) => source(theme, format!("game theme ({})", path.display()), rev),
        Err(e) => {
            eprintln!(
                "gui-builder: theme at {} is broken ({e}); using placeholder",
                path.display()
            );
            source(Theme::placeholder(), "placeholder theme".into(), rev)
        }
    }
}

fn source(theme: Theme, label: String, rev: u64) -> ThemeSource {
    let style_keys = theme.style_keys().map(str::to_owned).collect();
    ThemeSource {
        theme: Arc::new(theme),
        style_keys,
        label,
        rev,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_combo_source_is_the_themes_own_key_list() {
        // Whichever theme resolves (game kit or placeholder), the combo
        // source must be exactly Theme::style_keys().
        let src = load(&AssetRoots::new(None, Vec::new()), 0);
        let expect: Vec<String> = src.theme.style_keys().map(str::to_owned).collect();
        assert_eq!(src.style_keys, expect);
        assert!(!src.style_keys.is_empty(), "theme defines no parts?");
    }
}
