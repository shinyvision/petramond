//! The save's recorded mod set (`mods.json` in the save dir): active pack ids
//! + versions, written on every save and compared at world open with a LOUD
//! warning listing added / removed / version-changed mods. Nothing blocks —
//! content already degrades safely (the name-addressed save palette maps
//! unknown blocks to air, unknown mob species are skipped).
//!
//! Only id-bearing packs are recorded: a content-only override pack has no
//! namespace and introduces no name-addressed content of its own.
//!
//! The set records what is ENABLED for the world: packs the player disabled
//! per-world (`settings.json`) are excluded from both the record and the
//! comparison, so a deliberate disable/re-enable never trips the warning —
//! only genuine installs/removals/upgrades do.

use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ModSetEntry {
    pub id: String,
    /// The pack's declared version string (`""` when the pack declares none).
    #[serde(default)]
    pub version: String,
}

#[derive(Serialize, Deserialize, Default)]
struct ModsFile {
    mods: Vec<ModSetEntry>,
}

/// The ENABLED id-bearing packs (installed minus the world's disabled set),
/// sorted by id — the deterministic order the file is written in.
pub(crate) fn active(disabled: &BTreeSet<String>) -> Vec<ModSetEntry> {
    let mut mods: Vec<ModSetEntry> = crate::assets::packs()
        .iter()
        .filter_map(|p| {
            let id = p.id.clone()?;
            if disabled.contains(&id) {
                return None;
            }
            Some(ModSetEntry {
                id,
                version: p.version.clone().unwrap_or_default(),
            })
        })
        .collect();
    mods.sort_by(|a, b| a.id.cmp(&b.id));
    mods
}

/// The `mods.json` bytes for the current session's enabled set.
pub(crate) fn encode_active(disabled: &BTreeSet<String>) -> Vec<u8> {
    encode(active(disabled))
}

fn encode(mods: Vec<ModSetEntry>) -> Vec<u8> {
    serde_json::to_vec_pretty(&ModsFile { mods }).unwrap_or_default()
}

/// Compare the save's recorded mod set against the ENABLED one and warn
/// loudly on any difference. A missing `mods.json` (a fresh world, or one
/// last saved before mod-set recording existed) compares silently — the first
/// save writes it. Both sides exclude the world's deliberately disabled mods
/// (the record was written that way too), so per-world disables never warn.
/// Called at world open (`save::open_at`).
pub(crate) fn warn_on_mismatch(save_dir: &Path, disabled: &BTreeSet<String>) {
    let Ok(bytes) = std::fs::read(save_dir.join("mods.json")) else {
        return;
    };
    let recorded = match serde_json::from_slice::<ModsFile>(&bytes) {
        Ok(f) => f.mods,
        Err(e) => {
            log::warn!("save mods.json is unreadable ({e}); mod-set check skipped");
            return;
        }
    };
    // A record written before the mod was disabled would otherwise report it
    // MISSING every open; the player switched it off on purpose.
    let recorded: Vec<ModSetEntry> = recorded
        .into_iter()
        .filter(|r| !disabled.contains(&r.id))
        .collect();
    for line in diff(&recorded, &active(disabled)) {
        log::warn!("{line}");
    }
}

/// One human-readable warning line per difference between the save's recorded
/// mod set and the active one. Pure, for the unit test.
fn diff(recorded: &[ModSetEntry], active: &[ModSetEntry]) -> Vec<String> {
    let mut lines = Vec::new();
    for r in recorded {
        match active.iter().find(|a| a.id == r.id) {
            None => lines.push(format!(
                "mod '{}' (v{}) was active when this world was last saved but is MISSING now; \
                 its content degrades safely (blocks→air, mobs skipped) but its data is inert",
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
        }
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
}
