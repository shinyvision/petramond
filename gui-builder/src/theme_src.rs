//! Theme loading for the builder: the real game theme kit when it exists
//! (`assets/ui/theme/theme.json` in the repo), otherwise petramond-ui's
//! placeholder — another agent authors the real kit in parallel, so the
//! placeholder path must always work.

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

/// Candidate locations of the shipped theme manifest, relative to wherever
/// the builder runs from.
fn theme_manifest_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(repo) = manifest_dir.parent() {
        out.push(repo.join("assets/ui/theme/theme.json"));
    }
    out.push(PathBuf::from("assets/ui/theme/theme.json"));
    out.push(PathBuf::from("../assets/ui/theme/theme.json"));
    out
}

pub fn load(rev: u64) -> ThemeSource {
    for path in theme_manifest_candidates() {
        let Ok(json) = std::fs::read_to_string(&path) else {
            continue;
        };
        let dir = path.parent().map(PathBuf::from).unwrap_or_default();
        let read = |name: &str| std::fs::read(dir.join(name)).ok();
        match Theme::load(&json, &read) {
            Ok(theme) => return source(theme, format!("game theme ({})", path.display()), rev),
            Err(e) => {
                eprintln!(
                    "gui-builder: theme at {} is broken ({e}); using placeholder",
                    path.display()
                );
                break;
            }
        }
    }
    source(Theme::placeholder(), "placeholder theme".into(), rev)
}

fn source(theme: Theme, label: String, rev: u64) -> ThemeSource {
    let style_keys = theme.style_keys().map(str::to_owned).collect();
    ThemeSource { theme: Arc::new(theme), style_keys, label, rev }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_combo_source_is_the_themes_own_key_list() {
        // Whichever theme resolves (game kit or placeholder), the combo
        // source must be exactly Theme::style_keys().
        let src = load(0);
        let expect: Vec<String> = src.theme.style_keys().map(str::to_owned).collect();
        assert_eq!(src.style_keys, expect);
        assert!(!src.style_keys.is_empty(), "theme defines no parts?");
    }
}
