//! Every rig clip this pack plays is on its rig. The pack names engine clips
//! and resolves them only when it plays them, so a clip renamed or removed
//! in a rig would break the sword, the shield or the bow at runtime and
//! nowhere else. The rigs are read the way the engine reads them: the rigs
//! catalog's rows, each model's own clips under the engine's namespace, plus
//! the clips of every library its animator document names.

use std::path::{Path, PathBuf};

use crate::families::{Families, FAMILIES_JSON};
use crate::{bow, guard};
use mod_sdk::*;

fn assets() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")
}

fn document(path: &Path) -> json::Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    json::Value::parse(&text).unwrap_or_else(|| panic!("{}: not JSON", path.display()))
}

/// Every clip `rig` carries, by full name.
fn rig_clips(rig: &str) -> Vec<String> {
    let catalog = document(&assets().join("animations/rigs.json"));
    let row = catalog
        .get(rig)
        .unwrap_or_else(|| panic!("no rig row `{rig}`"));
    let field = |key: &str| {
        row.get(key)
            .and_then(json::Value::as_str)
            .expect(key)
            .to_string()
    };
    let model = document(&assets().join(field("model")));
    let mut clips: Vec<String> = model
        .get("animations")
        .and_then(json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|clip| clip.get("name").and_then(json::Value::as_str))
        .map(|name| format!("petramond:{name}"))
        .collect();
    let animator = assets().join(field("animator"));
    let libraries = document(&animator);
    for library in libraries
        .get("libraries")
        .and_then(json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(json::Value::as_str)
    {
        let library = document(&animator.parent().expect("a directory").join(library));
        if let Some(animations) = library.get("animations").and_then(json::Value::as_object) {
            clips.extend(
                animations
                    .iter()
                    .map(|(name, _)| format!("petramond:{name}")),
            );
        }
    }
    clips
}

#[test]
fn every_clip_the_pack_plays_is_on_its_rig() {
    let (families, refused) = Families::parse(FAMILIES_JSON);
    assert!(refused.is_empty(), "refused family rows: {refused:?}");
    let mut played: Vec<(&str, String)> = Vec::new();
    for style in families.styles() {
        let family = families.get(style);
        for motion in family.attacks.iter().chain([&family.work]) {
            played.push((rig::PLAYER_FIRST_PERSON, motion.first_person.clone()));
            played.push((rig::PLAYER_BODY, motion.body.clone()));
        }
    }
    for &(rig, clip) in guard::CLIPS.iter().chain(&bow::CLIPS) {
        played.push((rig, clip.to_string()));
    }

    let body = rig_clips(rig::PLAYER_BODY);
    let first_person = rig_clips(rig::PLAYER_FIRST_PERSON);
    let missing: Vec<String> = played
        .iter()
        .filter(|(rig, clip)| {
            let on = if *rig == rig::PLAYER_BODY {
                &body
            } else {
                &first_person
            };
            !on.contains(clip)
        })
        .map(|(rig, clip)| format!("{rig}: {clip}"))
        .collect();
    assert!(!played.is_empty());
    assert!(
        missing.is_empty(),
        "clips the pack plays that its rig lacks:\n{}",
        missing.join("\n")
    );
}
