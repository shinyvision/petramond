use crate::assets::AssetRoots;
use petramond_ui::{Theme, ThemeLayer};
use std::path::PathBuf;
use std::sync::Arc;

pub struct ThemeSource {
    pub theme: Arc<Theme>,
    pub style_keys: Vec<String>,
    pub label: String,
    pub rev: u64,
}

const THEME_JSON: &str = "ui/theme/theme.json";

pub fn load(roots: &AssetRoots, rev: u64) -> ThemeSource {
    let paths = roots.all(THEME_JSON);
    if paths.is_empty() {
        return source(Theme::placeholder(), "placeholder theme".into(), rev);
    }
    let label = paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(" + ");
    match load_stack(&paths) {
        Ok(theme) => source(theme, format!("game theme ({label})"), rev),
        Err(e) => {
            eprintln!("gui-builder: theme stack {label} is broken ({e}); using placeholder");
            source(Theme::placeholder(), "placeholder theme".into(), rev)
        }
    }
}

fn load_stack(paths: &[PathBuf]) -> Result<Theme, String> {
    let jsons = paths
        .iter()
        .map(|p| std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display())))
        .collect::<Result<Vec<_>, _>>()?;
    let readers: Vec<_> = paths
        .iter()
        .map(|p| {
            let dir = p.parent().map(PathBuf::from).unwrap_or_default();
            move |name: &str| std::fs::read(dir.join(name)).ok()
        })
        .collect();
    let layers: Vec<ThemeLayer<'_>> = jsons
        .iter()
        .zip(&readers)
        .map(|(json, read)| ThemeLayer { json, read })
        .collect();
    Theme::load_stack(&layers).map_err(|e| e.to_string())
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
        let src = load(&AssetRoots::new(None, Vec::new()), 0);
        let expect: Vec<String> = src.theme.style_keys().map(str::to_owned).collect();
        assert_eq!(src.style_keys, expect);
        assert!(!src.style_keys.is_empty(), "theme defines no parts?");
    }
}
