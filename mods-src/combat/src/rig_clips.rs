use std::path::{Path, PathBuf};

use crate::families::FamilySpec;
use crate::keys::FAMILY_DATA;
use crate::{bow, guard};
use mod_sdk::*;

fn assets() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets")
}

fn document(path: &Path) -> json::Value {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    json::Value::parse(&text).unwrap_or_else(|| panic!("{}: not JSON", path.display()))
}

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
    let authored = assets().join(field("animator"));
    let pack = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("pack")
        .join(field("animator"));
    for (namespace, animator) in [("petramond", authored), ("combat", pack)] {
        if !animator.exists() {
            continue;
        }
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
                        .map(|(name, _)| format!("{namespace}:{name}")),
                );
            }
        }
    }
    clips
}

#[test]
fn every_clip_the_pack_plays_is_on_its_rig() {
    let rows = pack_rows_with_data(include_str!("../pack/items.json"), "items", FAMILY_DATA);
    assert!(!rows.is_empty(), "the pack declares its families");
    let mut played: Vec<(&str, String)> = Vec::new();
    for (row, entry) in &rows {
        let spec: FamilySpec = parse_row_data(entry).unwrap_or_else(|e| panic!("{row}: {e}"));
        for (first_person, body) in spec.clips() {
            played.push((rig::PLAYER_FIRST_PERSON, first_person.to_owned()));
            played.push((rig::PLAYER_BODY, body.to_owned()));
        }
    }
    for &(rig, clip) in guard::CLIPS.iter().chain(&bow::CLIPS) {
        played.push((rig, clip.to_string()));
    }

    for (_, entry) in pack_rows_with_data(
        include_str!("../pack/items.json"),
        "items",
        crate::keys::BOOMERANG_KEY,
    ) {
        let entry = json::Value::parse(&entry).expect("boomerang data JSON");
        let clips = entry
            .get("animations")
            .expect("boomerang animation bindings");
        for (rig, key) in [
            (rig::PLAYER_FIRST_PERSON, "first_person"),
            (rig::PLAYER_BODY, "body"),
        ] {
            for clip in clips
                .get(key)
                .and_then(json::Value::as_array)
                .expect("draw and throw")
            {
                played.push((rig, clip.as_str().expect("clip name").to_owned()));
            }
        }
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
