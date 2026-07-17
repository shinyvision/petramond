//! Per-world settings: `settings.json` in the save dir.
//!
//! Currently one knob: `disabled_mods` — the pack ids the player switched OFF
//! for this world (World Settings screen). An absent file means "everything
//! enabled", so worlds created before this existed change nothing. The set is
//! consulted once at world open (`save::open_at` / `Game::new`); editing it
//! for a world that is not open takes effect on the next open (no live
//! reload). Serialization is deterministic: a `BTreeSet` encodes sorted.
//! Unknown fields are ignored, so files written by retired knobs (the old
//! `optimize_explored_terrain` toggle — explored terrain always persists now)
//! keep loading.
//!
//! Ids of packs that are no longer installed stay in the set untouched, so
//! reinstalling a mod does not silently re-enable it.

use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldSettings {
    /// Mod pack ids disabled for this world. Everything else is enabled.
    #[serde(default)]
    pub disabled_mods: BTreeSet<String>,
}

/// Read the world's settings. Absent file = defaults (all mods enabled); an
/// unreadable file warns and falls back to defaults — settings must never
/// block opening a world.
pub fn load(dir: &Path) -> WorldSettings {
    let path = dir.join("settings.json");
    let Ok(bytes) = std::fs::read(&path) else {
        return WorldSettings::default();
    };
    match serde_json::from_slice(&bytes) {
        Ok(s) => s,
        Err(e) => {
            log::warn!(
                "world settings {} are unreadable ({e}); using defaults (all mods enabled)",
                path.display()
            );
            WorldSettings::default()
        }
    }
}

/// Write the world's settings (atomic tmp+rename, like every save-dir file).
pub fn store(dir: &Path, settings: &WorldSettings) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let bytes = serde_json::to_vec_pretty(settings).map_err(std::io::Error::other)?;
    super::write_atomic(&dir.join("settings.json"), &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_roundtrip_and_absent_file_defaults() {
        let dir =
            std::env::temp_dir().join(format!("petramond-settings-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        // Absent file (directory doesn't even exist) = all mods enabled.
        assert_eq!(load(&dir), WorldSettings::default());
        assert!(load(&dir).disabled_mods.is_empty());

        // Roundtrip, with deterministic (sorted) serialization.
        let settings = WorldSettings {
            disabled_mods: ["zeta".to_owned(), "alpha".to_owned()]
                .into_iter()
                .collect(),
        };
        store(&dir, &settings).expect("settings write");
        assert_eq!(load(&dir), settings);

        let text = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(
            text.find("alpha").unwrap() < text.find("zeta").unwrap(),
            "encoding is sorted (deterministic): {text}"
        );

        // A file written by a retired knob (unknown field) still loads.
        std::fs::write(
            dir.join("settings.json"),
            br#"{ "disabled_mods": ["alpha"], "optimize_explored_terrain": false }"#,
        )
        .unwrap();
        assert!(load(&dir).disabled_mods.contains("alpha"));

        // A corrupt file degrades to defaults instead of blocking the open.
        std::fs::write(dir.join("settings.json"), b"{ nope").unwrap();
        assert_eq!(load(&dir), WorldSettings::default());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
