use super::*;

const HABITATS: &str = r#"{"underground_biomes":[
 {"underground_biome":"test:humid","climate":{"humidity":[0.7,1.0],"depth":[0.2,0.9]}},
 {"underground_biome":"test:deep","climate":{"erosion":[-1.0,-0.6],"depth":[1.08,1.2]}}
]}"#;

#[test]
fn underground_identity_uses_climate_fitness_and_depth() {
    let table = test_table(&[HABITATS]);
    let humid = table.id("test:humid").unwrap();
    let deep = table.id("test:deep").unwrap();
    assert_eq!(table.id_at([0.0, 0.8, 0.0, 0.0, 0.0, 0.5], 0), humid);
    assert_eq!(table.id_at([0.0, -0.5, 0.0, 0.0, 0.0, 0.5], 0), 0);
    assert_eq!(table.id_at([0.0, 0.8, 0.0, 0.0, 0.0, 0.0], 0), 0);
    assert_eq!(table.id_at([0.0, 0.0, 0.0, -0.8, 0.0, 1.1], 0), deep);
    // Ranges compete by distance, so their edges do not impose hard climate cutoffs.
    assert_eq!(table.id_at([0.0, 0.65, 0.0, 0.0, 0.0, 0.5], 0), humid);
}

#[test]
fn overlapping_climates_use_names_and_preserve_registry_ids() {
    let a =
        r#"{"underground_biomes":[{"underground_biome":"a:zone","climate":{"depth":[0.2,0.9]}}]}"#;
    let z = a.replace("a:zone", "z:zone");
    for layers in [[a, z.as_str()], [z.as_str(), a]] {
        let table = test_table(&layers);
        let id = table.id_at([0.0, 0.0, 0.0, 0.0, 0.0, 0.5], 0);
        assert_eq!(table.name(id), Some("a:zone"));
        assert_eq!(table.id("petramond:stone"), Some(0));
    }
}

#[test]
fn climate_box_pruning_never_excludes_a_possible_winner() {
    let table = test_table(&[HABITATS]);
    for center in [-0.8, -0.1, 0.7] {
        for depth in [0.0, 0.5, 1.05] {
            let bounds = [
                [-0.3, 0.3],
                [center - 0.2, center + 0.2],
                [-0.5, 0.5],
                [-0.9, -0.1],
                [-0.2, 0.2],
                [depth, depth + 0.15],
            ];
            let mut ids = IdSet::default();
            table.ids_in((-20, 20), bounds, &mut ids);
            for i in 0..729 {
                let mut bits = i;
                let point = bounds.map(|[lo, hi]| {
                    let t = (bits % 3) as f64 / 2.0;
                    bits /= 3;
                    lo + (hi - lo) * t
                });
                assert!(ids.contains(table.id_at(point, 0)));
            }
        }
    }
}

#[test]
fn climate_and_surface_changes_invalidate_generation_cache() {
    let original = test_table(&[HABITATS]).fingerprint;
    for altered in [
        HABITATS.replace("0.7", "0.6"),
        HABITATS.replace("1.08", "1.07"),
    ] {
        assert_ne!(original, test_table(&[&altered]).fingerprint);
    }
    let lining = r#"{"underground_biomes":[{"underground_biome":"test:x","climate":{"depth":[0.2,0.9]},"lining":{"block":"petramond:moss_block","faces":{"floor_depth":2,"floor_under":{"block":"petramond:stone"}}}}]}"#;
    assert_ne!(
        test_table(&[lining]).fingerprint,
        test_table(&[&lining.replace("petramond:stone", "petramond:marble")]).fingerprint
    );
}

#[test]
fn malformed_habitat_selectors_and_surface_rules_fail_loading() {
    let base = shipped_layer();
    for row in [
        r#"{"underground_biome":"test:x"}"#,
        r#"{"underground_biome":"petramond:stone","climate":{"depth":[0.2,0.9]}}"#,
        r#"{"underground_biome":"test:x","climate":{"depth":[0.9,0.2]}}"#,
        r#"{"underground_biome":"test:x","climate":{"humidity":[-3,1],"depth":[0.2,0.9]}}"#,
        r#"{"underground_biome":"test:x","climate":{"depth":[0.2,0.9]},"lining":{"block":"petramond:air"}}"#,
        r#"{"underground_biome":"test:x","climate":{"depth":[0.2,0.9]},"lining":{"block":"petramond:stone","faces":{"floor_depth":17}}}"#,
        r#"{"underground_biome":"test:x","climate":{"depth":[0.2,0.9]},"aquifer":{"level":-16,"barrier":"petramond:air"}}"#,
    ] {
        let text = format!("{{\"underground_biomes\":[{row}]}}");
        assert!(parse_layers(&[&base, &text]).is_err(), "accepted {row}");
    }
}
