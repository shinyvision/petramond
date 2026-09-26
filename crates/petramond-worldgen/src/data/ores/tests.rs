use super::*;
use petramond_world::chunk::{SECTION_MAX_CY, SECTION_MIN_CY, SECTION_SIZE};

/// The vein table as it was compiled into the scatter pass before it moved
/// to `ores.json` (block, salt, count, shape, Y band, depth ramp; every vein
/// hosted by stone), in its fixed placement order.
pub(crate) fn legacy_veins() -> Vec<OreVein> {
    const STONE: &[Block] = &[Block::Stone];
    let blob = |block, salt, count, size, y_min, y_max| OreVein {
        block,
        salt,
        count,
        shape: VeinShape::Blob { size },
        y_min,
        y_max,
        depth_ramp: None,
        hosts: STONE,
    };
    let grid3 = |block, salt, count, y_min, y_max, depth_ramp| OreVein {
        block,
        salt,
        count,
        shape: VeinShape::Grid3 { max_ore: 9 },
        y_min,
        y_max,
        depth_ramp,
        hosts: STONE,
    };
    vec![
        blob(Block::Dirt, 0xA1_0005, 9, 33, WORLD_MIN_Y, 130),
        blob(Block::Gravel, 0xA1_0006, 10, 33, WORLD_MIN_Y, 130),
        blob(Block::Tuff, 0xA1_0004, 6, 40, WORLD_MIN_Y, 0),
        blob(Block::CoalOre, 0xA1_0010, 14, 17, 16, 150),
        blob(Block::CopperOre, 0xA1_0012, 11, 10, -16, 96),
        grid3(Block::IronOre, 0xA1_0011, 34, WORLD_MIN_Y, 136, None),
        blob(Block::GoldOre, 0xA1_0013, 3, 9, WORLD_MIN_Y, 30),
        grid3(Block::DiamondOre, 0xA1_0016, 7, WORLD_MIN_Y, 16, Some(1.0)),
        blob(Block::Marble, 0xA1_0007, 8, 33, WORLD_MIN_Y, 130),
    ]
}

/// The compiled section-skip band the scatter pass used before its span
/// was derived from the loaded rows.
const LEGACY_Y_SPAN: (i32, i32) = (WORLD_MIN_Y, 154);

fn base() -> String {
    petramond_world::assets::read_base_text("ores.json")
        .expect("shipped ores.json")
        .0
}

/// The shipped table is the compiled one, row for row in placement order.
#[test]
fn shipped_rows_match_the_compiled_vein_table() {
    let table = parse_layers(&[&base()]).expect("shipped ores load");
    assert_eq!(table.veins, legacy_veins().as_slice());
}

/// The derived Y span gates exactly the sections the hand-derived constant
/// did (it is tighter, but only inside the top section either reaches).
#[test]
fn derived_span_gates_the_same_sections_as_the_compiled_constant() {
    let table = parse_layers(&[&base()]).expect("shipped ores load");
    let overlaps = |(lo, hi): (i32, i32), cy: i32| {
        let sec_lo = cy * SECTION_SIZE as i32;
        sec_lo <= hi && sec_lo + SECTION_SIZE as i32 > lo
    };
    for cy in SECTION_MIN_CY..=SECTION_MAX_CY {
        assert_eq!(
            overlaps(table.y_span, cy),
            overlaps(LEGACY_Y_SPAN, cy),
            "section cy {cy}"
        );
    }
    let widest = legacy_veins()
        .iter()
        .map(|v| v.shape.reach().0)
        .max()
        .unwrap();
    assert_eq!(table.max_reach, widest);
}

/// A pack adds an ore after the engine rows (so it never moves them) and may
/// retune an engine row in place; its hosts may be any blocks.
#[test]
fn packs_add_ores_after_the_engine_rows_and_retune_engine_rows() {
    let pack = r#"{"ores": [
        {"ore": "petramond:coal_ore", "block": "petramond:coal_ore", "salt": 10551312,
         "count": 20, "shape": {"blob": {"size": 17}}, "y": [16, 150]},
        {"ore": "mymod:tuff_gold", "block": "petramond:gold_ore", "salt": 77,
         "count": 4, "shape": {"grid3": {"max_ore": 3}}, "y": [-64, -10],
         "hosts": ["petramond:tuff", "petramond:stone"]}
    ]}"#;
    let table = parse_layers(&[&base(), pack]).expect("pack layer loads");
    let legacy = legacy_veins();
    assert_eq!(table.veins.len(), legacy.len() + 1);
    assert_eq!(
        table.veins[3].count, 20,
        "the engine coal row is retuned in place"
    );
    let added = table.veins.last().expect("pack row");
    assert_eq!(added.block, Block::GoldOre);
    assert_eq!(added.hosts, &[Block::Tuff, Block::Stone]);
    assert_eq!(added.shape, VeinShape::Grid3 { max_ore: 3 });
    for (i, (loaded, compiled)) in table.veins.iter().zip(&legacy).enumerate() {
        if i != 3 {
            assert_eq!(loaded, compiled, "engine row {i} untouched");
        }
    }
}

/// Rows the scatter pass could not place seamlessly or at all are refused.
#[test]
fn malformed_rows_are_refused() {
    let row = |fields: &str| {
        format!(
            r#"{{"ores": [{{"ore": "mymod:x", "block": "petramond:gold_ore", "salt": 1,
                "count": 2, {fields}}}]}}"#
        )
    };
    let base = base();
    let valid = row(r#""shape": {"blob": {"size": 9}}, "y": [0, 10]"#);
    assert!(parse_layers(&[&base, &valid]).is_ok());
    for bad in [
        r#""shape": {"blob": {"size": 9}}, "y": [10, 0]"#,
        r#""shape": {"blob": {"size": 9}}, "y": [-100, 10]"#,
        r#""shape": {"blob": {"size": 0}}, "y": [0, 10]"#,
        r#""shape": {"blob": {"size": 100000}}, "y": [0, 10]"#,
        r#""shape": {"grid3": {"max_ore": 10}}, "y": [0, 10]"#,
        r#""shape": {"blob": {"size": 9}}, "y": [0, 10], "depth_ramp": 2.0"#,
        r#""shape": {"blob": {"size": 9}}, "y": [5, 5], "depth_ramp": 0.5"#,
        r#""shape": {"blob": {"size": 9}}, "y": [0, 10], "hosts": []"#,
        r#""shape": {"blob": {"size": 9}}, "y": [0, 10], "hosts": ["petramond:nope"]"#,
        r#""shape": {"cube": {"size": 9}}, "y": [0, 10]"#,
        r#""shape": {"blob": {"size": 9}}, "y": [0, 10], "glow": true"#,
    ] {
        assert!(parse_layers(&[&base, &row(bad)]).is_err(), "accepted {bad}");
    }
}

/// A row without a pinned salt derives one from its namespaced name, so a
/// pack author never picks an integer; a salt two rows share is refused.
#[test]
fn salts_derive_from_names_and_never_repeat() {
    let pack = r#"{"ores": [{"ore": "mymod:tin", "block": "petramond:gold_ore",
        "count": 2, "shape": {"blob": {"size": 9}}, "y": [0, 10]}]}"#;
    let table = parse_layers(&[&base(), pack]).expect("an unsalted row loads");
    let tin = table.veins.last().expect("pack row");
    assert_eq!(tin.salt, crate::salts::named("ore", "mymod:tin"));

    let clash = r#"{"ores": [{"ore": "mymod:tin", "block": "petramond:gold_ore",
        "salt": 10551312, "count": 2, "shape": {"blob": {"size": 9}}, "y": [0, 10]}]}"#;
    let err = parse_layers(&[&base(), clash])
        .err()
        .expect("a row reusing coal's salt is refused");
    assert!(
        err.contains("petramond:coal_ore") && err.contains("mymod:tin"),
        "{err}"
    );
}
