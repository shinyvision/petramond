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

const POOL_ROW: &str = r#"{"fluid_pool":"test:lava","fluid":"petramond:lava","anchor_y":-40,
    "chance":0.5,"height_scale":18.0,"max_y":0,"reach":16,"max_depth":10,"max_drop":24,
    "max_sink":16,"budget":2000,"surface_clearance":32}"#;
const FALL_ROW: &str = r#"{"fluid_fall":"test:lava","fluid":"petramond:lava","chance":0.5,
    "y":[-38,-11],"min_surface":45}"#;

#[test]
fn a_later_layer_disables_a_fluid_row_by_zeroing_its_chance() {
    let rows = format!(r#"{{"fluid_pools":[{POOL_ROW}],"fluid_falls":[{FALL_ROW}]}}"#);
    let off = rows.replace("\"chance\":0.5", "\"chance\":0");
    let on = synthetic_table(&[&rows]);
    assert_eq!((on.pools.len(), on.falls.len()), (1, 1));
    let disabled = synthetic_table(&[&rows, &off]);
    assert_eq!((disabled.pools.len(), disabled.falls.len()), (0, 0));
}

#[test]
fn malformed_fluid_rows_and_files_fail_loading() {
    let pools = |row: String| format!(r#"{{"fluid_pools":[{row}]}}"#);
    let falls = |row: String| format!(r#"{{"fluid_falls":[{row}]}}"#);
    for text in [
        pools(POOL_ROW.replace("\"anchor_y\":-40", "\"anchor_y\":8")),
        pools(POOL_ROW.replace("test:lava\",\"fluid\"", "lava\",\"fluid\"")),
        pools(POOL_ROW.replace(
            "\"fluid\":\"petramond:lava\"",
            "\"fluid\":\"petramond:stone\"",
        )),
        falls(FALL_ROW.replace("\"min_surface\":45", "\"min_surface\":100000")),
        falls(FALL_ROW.replace("\"min_surface\":45", "\"min_surface\":10")),
        format!(r#"{{"fluid_pool":[{POOL_ROW}]}}"#),
    ] {
        assert!(
            parse_layers(&[&shipped_layer(), &text]).is_err(),
            "accepted {text}"
        );
    }
}
