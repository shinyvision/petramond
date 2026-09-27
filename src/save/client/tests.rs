use super::*;

#[test]
fn identity_fields_roundtrip_and_default_to_none() {
    let dir = petramond_util::test_dirs::TestScratchDir::new("clienttest-identity");
    let file = dir.join("client.json");

    let settings = ClientSettings {
        player_name: Some("Rachel".to_string()),
        last_server: Some("host:7434".to_string()),
        anti_aliasing: AntiAliasing::Off,
        ..ClientSettings::default()
    };
    store_to(&file, &settings).expect("store");
    assert_eq!(load_from(&file), settings);

    std::fs::write(&file, br#"{ "fps_cap": 90 }"#).expect("write old-style file");
    let old = load_from(&file);
    assert_eq!(old.player_name, None);
    assert_eq!(old.last_server, None);
    assert_eq!(old.fps_cap, 90);
}

#[test]
fn player_name_resolution_trims_and_falls_through_blanks() {
    assert_eq!(
        first_nonempty([None, Some("  ".into()), Some(" Rachel ".into())]),
        "Rachel"
    );
    assert_eq!(first_nonempty([None, Some(String::new())]), "Player");
}

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
