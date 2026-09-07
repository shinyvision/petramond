use super::*;

#[test]
fn identity_fields_roundtrip_and_default_to_none() {
    let dir = std::env::temp_dir().join(format!(
        "petramond-clienttest-{}-identity",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let file = dir.join("client.json");

    // New fields survive a store/load round-trip.
    let settings = ClientSettings {
        player_name: Some("Rachel".to_string()),
        last_server: Some("host:7434".to_string()),
        anti_aliasing: AntiAliasing::Off,
        ..ClientSettings::default()
    };
    store_to(&file, &settings).expect("store");
    assert_eq!(load_from(&file), settings);

    // A file from before the fields existed (no such keys) loads as None
    // and keeps its other values.
    std::fs::write(&file, br#"{ "fps_cap": 90 }"#).expect("write old-style file");
    let old = load_from(&file);
    assert_eq!(old.player_name, None);
    assert_eq!(old.last_server, None);
    assert_eq!(old.fps_cap, 90);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn player_name_resolution_trims_and_falls_through_blanks() {
    // Pure precedence check (the env layers feed the same chain — see
    // `resolve_player_name`): blank/whitespace candidates fall through,
    // the first real one wins trimmed, and nothing left means "Player".
    assert_eq!(
        first_nonempty([None, Some("  ".into()), Some(" Rachel ".into())]),
        "Rachel"
    );
    assert_eq!(first_nonempty([None, Some(String::new())]), "Player");
}
