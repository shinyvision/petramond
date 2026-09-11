use super::*;

const ROOM: &str = r#"{"excavations":[
 {"excavation":"test:room","placement":{"spacing":96,"y":[-48,32]},
  "chamber":{"radius":[8,12],"feather":9,"lobes":2,"lobe_spread":0.4,
             "lobe_scale":[0.6,0.8],"tunnel":1}}
]}"#;

#[test]
fn invalid_placement_and_detached_geometry_are_rejected() {
    let biomes = super::super::underground::test_table(&[]);
    for (from, to) in [
        ("\"spacing\":96", "\"spacing\":24"),
        ("\"spacing\":96", "\"spacing\":95"),
        ("\"y\":[-48,32]", "\"y\":[-48,-40]"),
        ("\"feather\":9", "\"feather\":2"),
        ("\"lobe_spread\":0.4", "\"lobe_spread\":1"),
        ("\"spacing\":96", "\"spacing\":96,\"one_in\":0"),
        (
            "\"spacing\":96",
            "\"spacing\":96,\"underground_biome\":\"test:missing\"",
        ),
    ] {
        assert!(
            load::parse_layers(&[&ROOM.replace(from, to)], biomes).is_err(),
            "accepted {to}"
        );
    }
}

#[test]
fn independent_rows_keep_canonical_order_and_cache_identity() {
    let biomes = super::super::underground::test_table(&[]);
    let other = ROOM.replace("test:room", "aaa:room");
    let a = load::parse_layers(&[ROOM, &other], biomes).unwrap();
    let b = load::parse_layers(&[&other, ROOM], biomes).unwrap();
    assert_eq!(a.fingerprint, b.fingerprint);
    assert_eq!(
        a.rows.iter().map(|r| r.salt).collect::<Vec<_>>(),
        b.rows.iter().map(|r| r.salt).collect::<Vec<_>>()
    );
    let original = load::parse_layers(&[ROOM], biomes).unwrap().fingerprint;
    for (from, to) in [
        ("\"spacing\":96", "\"spacing\":128"),
        ("\"radius\":[8,12]", "\"radius\":[8,13]"),
        ("\"tunnel\":1", "\"tunnel\":2"),
        (
            "\"spacing\":96",
            "\"spacing\":96,\"contact\":\"natural_cave\"",
        ),
        ("\"y\":[-48,32]", "\"y\":[-48,40]"),
        (
            "\"spacing\":96",
            "\"spacing\":96,\"underground_biome\":\"petramond:stone\"",
        ),
    ] {
        assert_ne!(
            original,
            load::parse_layers(&[&ROOM.replace(from, to)], biomes)
                .unwrap()
                .fingerprint
        );
    }
}

#[test]
fn connected_geometry_validates_its_bounds_and_changes_cache_identity() {
    let biomes = super::super::underground::test_table(&[]);
    let source = ROOM.replace("\"chamber\":", "\"connections\":{\"radius\":[3,5],\"flatten\":0.8,\"feather\":8,\"bend\":0.2},\"chamber\":");
    let parsed = load::parse_layers(&[&source], biomes).unwrap();
    assert_ne!(
        parsed.fingerprint,
        load::parse_layers(&[ROOM], biomes).unwrap().fingerprint
    );
    for (from, to) in [
        ("\"radius\":[3,5]", "\"radius\":[5,3]"),
        ("\"feather\":8", "\"feather\":0"),
        ("\"bend\":0.2", "\"bend\":2"),
        ("\"y\":[-48,32]", "\"y\":[-48,-38]"),
    ] {
        assert!(load::parse_layers(&[&source.replace(from, to)], biomes).is_err());
    }
    for (from, to) in [
        ("\"radius\":[3,5]", "\"radius\":[3,6]"),
        ("\"flatten\":0.8", "\"flatten\":0.9"),
        ("\"feather\":8", "\"feather\":9"),
        ("\"bend\":0.2", "\"bend\":0.3"),
    ] {
        assert_ne!(
            parsed.fingerprint,
            load::parse_layers(&[&source.replace(from, to)], biomes)
                .unwrap()
                .fingerprint
        );
    }
}
