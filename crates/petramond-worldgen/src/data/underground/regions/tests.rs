use super::*;

const REGION: &str = r#"{"underground_biomes":[{
    "underground_biome":"test:region",
    "climate":{"continentality":[0.2,0.7],"depth":[0.25,1.25]},
    "region":{"shape":{"radius":128,"separation":256,"jitter":0,"warp":0,"warp_seeds":[1,2]},"bucket":[0,1]}
}]}"#;

#[test]
fn region_gates_sample_the_center_and_preserve_the_whole_depth_range() {
    let table = super::super::test_table(&[REGION]);
    let id = table.id("test:region").unwrap();
    assert_eq!(table.id_at([0.0, 0.0, 0.4, 0.0, 0.0, 0.5], 0), 0);
    for quart in [[0, 0], [20, 0], [-20, 0]] {
        let mut candidates = IdSet::default();
        table.regions[0].candidates(
            23,
            quart,
            |center| {
                assert_eq!(center, [0, 0]);
                [0.0, 0.0, 0.4, 0.0, 0.0, 64.0]
            },
            &mut candidates,
        );
        assert!(candidates.contains(id));
        for y in [-96, -64, -32, 0, 32, 35] {
            assert_eq!(table.region_id(candidates, 64.0, y), Some(id));
        }
        for y in [-100, 36, 64] {
            assert_eq!(table.region_id(candidates, 64.0, y), None);
        }
        let mut bounded = IdSet::default();
        table.region_ids_in(candidates, 64.0, [-64, 63], &mut bounded);
        assert!(bounded.contains(id));
        let mut above = IdSet::default();
        table.region_ids_in(candidates, 64.0, [36, 63], &mut above);
        assert!(!above.contains(id));
    }
}

#[test]
fn spacing_and_climate_reject_regions_without_nearest_fitness_leakage() {
    let table = super::super::test_table(&[REGION]);
    let mut outside = IdSet::default();
    table.regions[0].candidates(23, [33, 0], |_| panic!("outside the region"), &mut outside);
    assert!(outside.iter().next().is_none());
    let mut wrong_climate = IdSet::default();
    table.regions[0].candidates(23, [0, 0], |_| [0.0; 6], &mut wrong_climate);
    assert!(wrong_climate.iter().next().is_none());
}
