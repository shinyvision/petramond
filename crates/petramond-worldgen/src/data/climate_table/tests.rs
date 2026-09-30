use super::*;
use crate::biome::climate::SurfaceClimate;

fn base() -> String {
    petramond_world::assets::read_base_text("climate_table.json")
        .expect("shipped climate_table.json")
        .0
}

#[test]
fn pack_rows_claim_their_niche_ahead_of_the_base_table() {
    let niche = r#"{"bands": {"humidity": {"soaked": [0.9, 1.0]}},
        "groups": [{"rows": [{"temperature": "temperate", "humidity": "soaked",
            "continentality": "mid_inland..far_inland", "erosion": "full",
            "variance": "full", "biome": "petramond:wetland"}]}]}"#;
    let base = base();
    let layered = parse_layers(&[&base, niche]).expect("pack layer loads");
    let shipped = parse_layers(&[&base]).expect("shipped table loads");
    assert_eq!(layered.rows.len(), shipped.rows.len() + 1);
    assert_eq!(layered.rows[0].1, Biome::WETLAND, "the pack row leads");
    let index = BiomeClimateIndex::from_rects(&layered.rows);
    let inside = SurfaceClimate::new(0.0, 0.95, 0.5, 0.0, 0.3);
    let outside = SurfaceClimate::new(0.0, 0.5, 0.5, 0.0, 0.3);
    let base_index = BiomeClimateIndex::from_rects(&shipped.rows);
    assert_eq!(index.classify_surface(inside), Some(Biome::WETLAND));
    assert_ne!(base_index.classify_surface(inside), Some(Biome::WETLAND));
    assert_eq!(
        index.classify_surface(outside),
        base_index.classify_surface(outside)
    );

    let replaced = parse_layers(&[&base, &niche.replacen('{', r#"{"replace": true, "#, 1)])
        .expect("a replacing layer loads");
    assert_eq!(replaced.rows.len(), 1);
    let only = BiomeClimateIndex::from_rects(&replaced.rows);
    assert_eq!(only.classify_surface(outside), Some(Biome::WETLAND));
}

#[test]
fn a_pack_band_override_retunes_the_base_rows() {
    let retune = r#"{"bands": {"temperature": {
        "frozen": [-1.0, -0.25], "cold": [-0.25, -0.15]}}}"#;
    let table = parse_layers(&[&base(), retune]).expect("band override loads");
    assert_eq!(table.frozen_temperature_max, -0.25);
    let shipped = parse_layers(&[&base()]).expect("shipped table loads");
    assert_eq!(table.rows.len(), shipped.rows.len());
    let moved = table
        .rows
        .iter()
        .zip(&shipped.rows)
        .filter(|(a, b)| a != b)
        .count();
    assert!(moved > 0, "rows naming frozen/cold follow the override");
}

#[test]
fn malformed_tables_are_refused() {
    let bands = r#""bands": {
        "temperature": {"frozen": [-1.0, -0.3], "full": [-1.0, 1.0]},
        "humidity": {"full": [-1.0, 1.0]}, "continentality": {"full": [-1.0, 1.0]},
        "erosion": {"full": [-1.0, 1.0]}, "variance": {"full": [-1.0, 1.0]}}"#;
    let with = |groups: &str| format!(r#"{{{bands}, "groups": [{groups}]}}"#);
    let row = |fields: &str| {
        with(&format!(
            r#"{{"rows": [{{"temperature": "full", "humidity": "full",
                "continentality": "full", "erosion": "full"{fields}}}]}}"#
        ))
    };
    assert!(parse_layers(&[&row(r#", "variance": "full", "biome": "petramond:plains""#)]).is_ok());
    for (bad, why) in [
        (row(r#", "biome": "petramond:plains""#), "variance unset"),
        (
            with(
                r#"{"at": {"variance": "full"}, "rows": [{"temperature": "full",
                "humidity": "full", "continentality": "full", "erosion": "full",
                "variance": "full", "biome": "petramond:plains"}]}"#,
            ),
            "variance set twice",
        ),
        (
            row(r#", "variance": "full", "biome": "petramond:nowhere""#),
            "unknown biome",
        ),
        (
            row(r#", "variance": "full", "biome": "plains""#),
            "bare biome key",
        ),
        (
            row(r#", "variance": "wide", "biome": "petramond:plains""#),
            "unknown band",
        ),
        (
            row(r#", "variance": "full", "biome": "petramond:plains", "tilt": 1"#),
            "unknown field",
        ),
        (
            with(
                r#"{"grid": {"temperature": ["full"], "humidity": ["full"]},
                "at": {"variance": "full", "erosion": "full", "continentality": "full"},
                "rows": [{"biomes": [["petramond:plains", "petramond:forest"]]}]}"#,
            ),
            "grid row wider than the humidity list",
        ),
        (
            with(
                r#"{"grid": {"temperature": ["full"], "humidity": ["full"]},
                "at": {"variance": "full", "erosion": "full", "continentality": "full"},
                "rows": [{"biome": "petramond:plains"}]}"#,
            ),
            "grid row without a biomes grid",
        ),
        (r#"{"groups": []}"#.to_owned(), "no rows"),
        (
            row(r#", "variance": "full", "biome": "petramond:plains""#)
                .replace(r#""frozen": [-1.0, -0.3], "#, ""),
            "no frozen band",
        ),
    ] {
        assert!(parse_layers(&[&bad]).is_err(), "accepted: {why}");
    }
}

#[test]
fn the_fingerprint_tracks_placement() {
    let shipped = parse_layers(&[&base()]).expect("shipped table loads");
    let mut moved = parse_layers(&[&base()]).expect("shipped table loads");
    moved.rows[0].1 = Biome::MEADOW;
    assert_ne!(shipped.fingerprint(), moved.fingerprint());
    assert_eq!(
        shipped.fingerprint(),
        parse_layers(&[&base()]).unwrap().fingerprint()
    );
}
