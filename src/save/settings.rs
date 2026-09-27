use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldSettings {
    #[serde(default)]
    pub disabled_mods: BTreeSet<String>,
    #[serde(default)]
    pub keep_inventory: bool,
    #[serde(default)]
    pub auto_open_lan: bool,
    #[serde(default = "default_day_minutes")]
    pub day_minutes: u32,
}

pub const DEFAULT_DAY_MINUTES: u32 = 15;

fn default_day_minutes() -> u32 {
    DEFAULT_DAY_MINUTES
}

impl Default for WorldSettings {
    fn default() -> Self {
        Self {
            disabled_mods: BTreeSet::new(),
            keep_inventory: false,
            auto_open_lan: false,
            day_minutes: DEFAULT_DAY_MINUTES,
        }
    }
}

pub fn load(dir: &Path) -> WorldSettings {
    let mut settings = read(dir);
    first_sight(
        &mut settings,
        crate::modding::modset::recorded_ids(dir).as_ref(),
        &crate::content::held_off_at_first_sight(),
    );
    settings
}

pub fn load_persisting(dir: &Path) -> WorldSettings {
    let settings = load(dir);
    if settings != read(dir) {
        if let Err(e) = store(dir, &settings) {
            log::warn!("could not write {}/settings.json: {e}", dir.display());
        }
    }
    settings
}

pub fn first_sight(
    settings: &mut WorldSettings,
    recorded: Option<&BTreeSet<String>>,
    held: &BTreeSet<String>,
) -> bool {
    let Some(recorded) = recorded else {
        return false;
    };
    let mut changed = false;
    for id in held {
        if !recorded.contains(id) && !settings.disabled_mods.contains(id) {
            settings.disabled_mods.insert(id.clone());
            changed = true;
        }
    }
    changed
}

fn read(dir: &Path) -> WorldSettings {
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

pub fn store(dir: &Path, settings: &WorldSettings) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let bytes = serde_json::to_vec_pretty(settings).map_err(std::io::Error::other)?;
    petramond_persist::atomic_file::replace(&dir.join("settings.json"), &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_roundtrip_and_absent_file_defaults() {
        let scratch = petramond_util::test_dirs::TestScratchDir::new("settings-test");
        let dir = scratch.join("world");

        assert_eq!(load(&dir), WorldSettings::default());
        assert!(load(&dir).disabled_mods.is_empty());

        let settings = WorldSettings {
            disabled_mods: ["zeta".to_owned(), "alpha".to_owned()]
                .into_iter()
                .collect(),
            keep_inventory: true,
            auto_open_lan: true,
            day_minutes: 30,
        };
        store(&dir, &settings).expect("settings write");
        assert_eq!(load(&dir), settings);

        let text = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(
            text.find("alpha").unwrap() < text.find("zeta").unwrap(),
            "encoding is sorted (deterministic): {text}"
        );

        std::fs::write(
            dir.join("settings.json"),
            br#"{ "disabled_mods": ["alpha"], "optimize_explored_terrain": false }"#,
        )
        .unwrap();
        assert!(load(&dir).disabled_mods.contains("alpha"));

        std::fs::write(dir.join("settings.json"), b"{ nope").unwrap();
        assert_eq!(load(&dir), WorldSettings::default());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_library_pack_starts_off_only_in_a_world_that_never_saw_it() {
        let held: BTreeSet<String> = ["world_blocks".to_owned()].into();
        let mut settings = WorldSettings::default();
        assert!(
            !first_sight(&mut settings, None, &held),
            "a save without a record is left alone"
        );
        let seen: BTreeSet<String> = ["world_blocks".to_owned()].into();
        assert!(!first_sight(&mut settings, Some(&seen), &held));
        assert!(
            settings.disabled_mods.is_empty(),
            "a recorded pack stays on"
        );
        let before: BTreeSet<String> = ["forge".to_owned()].into();
        assert!(first_sight(&mut settings, Some(&before), &held));
        assert!(settings.disabled_mods.contains("world_blocks"));
        assert!(
            !first_sight(&mut settings, Some(&before), &held),
            "the fold is the same answer read twice"
        );
    }
}
