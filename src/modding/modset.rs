use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModSetEntry {
    pub id: String,
    #[serde(default)]
    pub version: String,
    #[serde(default = "affects_world_unknown")]
    pub affects_world: bool,
}

fn affects_world_unknown() -> bool {
    true
}

#[derive(Serialize, Deserialize, Default)]
struct ModsFile {
    mods: Vec<ModSetEntry>,
}

pub fn active(disabled: &BTreeSet<String>) -> Vec<ModSetEntry> {
    let mut mods: Vec<ModSetEntry> = petramond_world::assets::packs()
        .iter()
        .filter_map(|p| {
            let id = p.id.clone()?;
            if disabled.contains(&id) {
                return None;
            }
            Some(ModSetEntry {
                id,
                version: p.version.clone().unwrap_or_default(),
                affects_world: p.touches_world,
            })
        })
        .collect();
    mods.sort_by(|a, b| a.id.cmp(&b.id));
    mods
}

pub fn encode_active(disabled: &BTreeSet<String>) -> Vec<u8> {
    encode(active(disabled))
}

fn encode(mods: Vec<ModSetEntry>) -> Vec<u8> {
    serde_json::to_vec_pretty(&ModsFile { mods }).unwrap_or_default()
}

pub fn check_at_open(save_dir: &Path, disabled: &BTreeSet<String>) -> Vec<ModSetEntry> {
    let Some(recorded) = recorded_enabled(save_dir, disabled) else {
        return Vec::new();
    };
    for line in diff(&recorded, &active(disabled)) {
        log::warn!("{line}");
    }
    absent(recorded, &active(&BTreeSet::new()))
}

pub fn recorded_ids(save_dir: &Path) -> Option<BTreeSet<String>> {
    let bytes = std::fs::read(save_dir.join("mods.json")).ok()?;
    let file = serde_json::from_slice::<ModsFile>(&bytes).ok()?;
    Some(file.mods.into_iter().map(|m| m.id).collect())
}

fn recorded_enabled(save_dir: &Path, disabled: &BTreeSet<String>) -> Option<Vec<ModSetEntry>> {
    let bytes = std::fs::read(save_dir.join("mods.json")).ok()?;
    let recorded = match serde_json::from_slice::<ModsFile>(&bytes) {
        Ok(f) => f.mods,
        Err(e) => {
            log::warn!("save mods.json is unreadable ({e}); mod-set check skipped");
            return None;
        }
    };
    Some(
        recorded
            .into_iter()
            .filter(|r| !disabled.contains(&r.id))
            .collect(),
    )
}

pub fn missing(save_dir: &Path, disabled: &BTreeSet<String>) -> Vec<ModSetEntry> {
    match recorded_enabled(save_dir, disabled) {
        Some(recorded) => absent(recorded, &active(&BTreeSet::new())),
        None => Vec::new(),
    }
}

fn absent(recorded: Vec<ModSetEntry>, installed: &[ModSetEntry]) -> Vec<ModSetEntry> {
    recorded
        .into_iter()
        .filter(|r| !installed.iter().any(|a| a.id == r.id))
        .collect()
}

pub fn diff(recorded: &[ModSetEntry], active: &[ModSetEntry]) -> Vec<String> {
    let mut lines = Vec::new();
    for r in recorded {
        match active.iter().find(|a| a.id == r.id) {
            None => lines.push(format!(
                "mod '{}' (v{}) was active when this world was last saved but is MISSING now; \
                 its blocks, items and mobs are kept but not placed, usable or spawned until \
                 it returns",
                r.id, r.version
            )),
            Some(a) if a.version != r.version => lines.push(format!(
                "mod '{}' changed version since this world was last saved: v{} -> v{}",
                r.id, r.version, a.version
            )),
            Some(_) => {}
        }
    }
    for a in active {
        if !recorded.iter().any(|r| r.id == a.id) {
            lines.push(format!(
                "mod '{}' (v{}) is newly active for this world (not in its last-saved mod set)",
                a.id, a.version
            ));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, version: &str) -> ModSetEntry {
        ModSetEntry {
            id: id.into(),
            version: version.into(),
            affects_world: true,
        }
    }

    #[test]
    fn missing_packs_default_to_world_affecting_and_skip_the_disabled() {
        let dir = petramond_util::test_dirs::TestScratchDir::new("modset-missing");
        std::fs::write(
            dir.join("mods.json"),
            r#"{"mods": [{"id": "gone", "version": "1"}, {"id": "off", "version": "2"}]}"#,
        )
        .unwrap();
        let disabled: BTreeSet<String> = ["off".to_owned()].into();
        let missing = missing(&dir, &disabled);
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].id, "gone");
        assert!(missing[0].affects_world);
    }

    #[test]
    fn diff_reports_added_removed_and_version_changed() {
        let recorded = [entry("daynight", "1.0"), entry("wheel", "0.2")];
        let active = [entry("daynight", "1.1"), entry("zombies", "0.1")];
        let lines = diff(&recorded, &active);
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert!(
            lines
                .iter()
                .any(|l| l.contains("wheel") && l.contains("MISSING")),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("daynight") && l.contains("v1.0 -> v1.1")),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("zombies") && l.contains("newly active")),
            "{lines:?}"
        );
        assert!(
            diff(&recorded, &recorded).is_empty(),
            "identical sets are silent"
        );
    }

    #[test]
    fn mods_file_roundtrips_through_json() {
        let mods = vec![entry("a_mod", ""), entry("b_mod", "2.3")];
        let bytes = encode(mods.clone());
        let back: ModsFile = serde_json::from_slice(&bytes).expect("parses");
        assert_eq!(back.mods, mods);
    }

    #[test]
    fn only_mods_absent_from_the_active_set_are_missing() {
        let recorded = vec![entry("daynight", "1.0"), entry("wheel", "0.2")];
        let active = [entry("daynight", "1.1"), entry("zombies", "0.1")];
        assert_eq!(
            absent(recorded.clone(), &active),
            [entry("wheel", "0.2")],
            "a version change is not a removal"
        );
        assert!(absent(recorded.clone(), &recorded).is_empty());
    }
}
