use super::*;

#[test]
fn identity_fields_roundtrip_and_default_to_none() {
    let dir = petramond_util::test_dirs::TestScratchDir::new("clienttest-identity");
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

/// A signed-in Petramond account IS the local player's name — in singleplayer
/// too, which is where it first shipped wrong: the world's chat kept showing
/// the OS username after signing in. The account outranks client.json, and a
/// blank or absent one falls through to it rather than blanking the name.
///
/// Env is not exercised here: `PETRAMOND_PLAYER_NAME` is process-global state
/// no test may set for the rest of the suite. Its position (above the account,
/// as the deliberate per-run override) is documented on `resolve_player_name`.
#[test]
fn a_signed_in_account_outranks_the_configured_player_name() {
    let configured = ClientSettings {
        player_name: Some("rachel".to_string()),
        ..ClientSettings::default()
    };
    assert_eq!(
        player_name_from(&configured, Some(" Explorer ".to_string())),
        "Explorer",
        "the account username wins, trimmed"
    );
    assert_eq!(
        player_name_from(&configured, None),
        "rachel",
        "signed out, the configured name still answers"
    );
    assert_eq!(
        player_name_from(&configured, Some("   ".to_string())),
        "rachel",
        "a blank account name falls through instead of blanking the identity"
    );
}
