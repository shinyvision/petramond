use super::*;

const ORGANIC: &str =
    r#"{"set": "petramond:organic", "mask": "organic_transition_mask", "width_texels": 6}"#;

fn pair(name: &str, set: &str, a: &str, b: &str) -> String {
    format!(
        r#"{{"pair": "test:{name}", "set": "{set}", "blocks": ["petramond:{a}", "petramond:{b}"]}}"#
    )
}

fn layer(sets: &[&str], pairs: &[String]) -> String {
    format!(
        r#"{{"sets": [{}], "pairs": [{}]}}"#,
        sets.join(","),
        pairs.join(",")
    )
}

fn organic(pairs: &[(&str, &str)]) -> String {
    layer(
        &[ORGANIC],
        &pairs
            .iter()
            .map(|(a, b)| pair(&format!("{a}_{b}"), "petramond:organic", a, b))
            .collect::<Vec<_>>(),
    )
}

fn local(r: &Rules, set: u8, b: Block) -> u8 {
    r.local(set, b.id())
}

#[test]
fn explicit_pairs_are_symmetric_but_not_transitive() {
    let r = Rules::from_layers(&[&organic(&[("dirt", "grass"), ("grass", "sand")])]).unwrap();
    let set = &r.sets[0];
    let [a, b, c] = [Block::Dirt, Block::Grass, Block::Sand].map(|b| local(&r, 0, b));
    assert!(set.allows(a, b) && set.allows(b, a));
    assert!(set.allows(b, c) && set.allows(c, b));
    assert!(!set.allows(a, c) && !set.allows(c, a));
    assert!(!set.allows(a, a) && !set.allows(a, 0));
    assert!(!r.is_material(Block::Stone.id()));
    assert!(r.memberships(u16::MAX).is_empty());
}

#[test]
fn ids_are_independent_of_row_order_and_pair_direction() {
    let a = Rules::from_layers(&[&organic(&[("dirt", "grass"), ("grass", "sand")])]).unwrap();
    let b = Rules::from_layers(&[&organic(&[("sand", "grass"), ("grass", "dirt")])]).unwrap();
    for block in [Block::Dirt, Block::Grass, Block::Sand] {
        assert_eq!(a.memberships(block.id()), b.memberships(block.id()));
    }
    assert_eq!(a.sets[0].pairs, b.sets[0].pairs);
}

#[test]
fn layers_add_pairs_and_patches_retire_them() {
    let base = organic(&[("dirt", "grass"), ("grass", "sand")]);
    let pack = layer(
        &[],
        &[
            pair("dirt_sand", "petramond:organic", "dirt", "sand"),
            r#"{"patch": "test:grass_sand", "data": {"petramond:enabled": false}}"#.into(),
        ],
    );
    let r = Rules::from_layers(&[&base, &pack]).unwrap();
    let set = &r.sets[0];
    let [dirt, grass, sand] = [Block::Dirt, Block::Grass, Block::Sand].map(|b| local(&r, 0, b));
    assert!(set.allows(dirt, sand), "a pack layer adds a pair");
    assert!(!set.allows(grass, sand), "a patch retires a pair");
    assert!(set.allows(dirt, grass), "untouched pairs survive");
    let restated = layer(
        &[],
        &[pair("dirt_grass", "petramond:organic", "dirt", "clay")],
    );
    let r = Rules::from_layers(&[&base, &restated]).unwrap();
    assert!(
        r.is_material(Block::Clay.id()),
        "a same-key row replaces the earlier one"
    );
    assert!(!r.sets[0].allows(r.local(0, Block::Dirt.id()), r.local(0, Block::Grass.id())));
}

#[test]
fn a_block_may_join_several_sets_with_independent_local_ids() {
    let hard = r#"{"set": "test:hard", "mask": "organic_transition_mask", "width_texels": 2}"#;
    let text = layer(
        &[ORGANIC, hard],
        &[
            pair("dirt_stone", "petramond:organic", "dirt", "stone"),
            pair("stone_gravel", "test:hard", "stone", "gravel"),
        ],
    );
    let r = Rules::from_layers(&[&text]).unwrap();
    assert_eq!(r.sets.len(), 2);
    assert_eq!(r.sets[0].name, "petramond:organic", "sets sort by name");
    let stone: Vec<_> = r.memberships(Block::Stone.id()).to_vec();
    assert_eq!(stone.len(), 2);
    assert_eq!(stone[0].set, 0);
    assert_eq!(stone[1].set, 1);
    assert_eq!(
        r.local(1, Block::Dirt.id()),
        0,
        "dirt is not a hard material"
    );
    assert!(r.sets[1].allows(
        r.local(1, Block::Stone.id()),
        r.local(1, Block::Gravel.id())
    ));
    assert_eq!(r.sets[1].width_texels, 2);
}

#[test]
fn invalid_policy_fails_before_meshing() {
    for text in [
        organic(&[("dirt", "dirt")]),
        organic(&[("dirt", "grass"), ("grass", "dirt")]),
        organic(&[("dirt", "air")]),
        organic(&[("dirt", "missing_block")]),
        organic(&[("dirt", "oak_log")]),
        layer(&[], &[pair("x", "petramond:organic", "dirt", "grass")]),
        layer(
            &[ORGANIC],
            &[r#"{"patch": "test:nothing", "data": {"petramond:enabled": false}}"#.into()],
        ),
        layer(
            &[ORGANIC],
            &[format!(
                r#"{{"pair": "test:p", "set": "petramond:organic", "blocks": ["petramond:dirt", "petramond:grass"], "data": {{"{}": "no"}}}}"#,
                crate::registry::ENABLED_KEY
            )],
        ),
        layer(
            &[
                r#"{"set": "petramond:organic", "mask": "organic_transition_mask", "width_texels": 9}"#,
            ],
            &[pair("p", "petramond:organic", "dirt", "grass")],
        ),
        layer(
            &[r#"{"set": "petramond:organic", "mask": "dirt", "width_texels": 6}"#],
            &[pair("p", "petramond:organic", "dirt", "grass")],
        ),
    ] {
        assert!(Rules::from_layers(&[&text]).is_err(), "{text}");
    }
}

#[test]
fn caps_are_reported_with_the_offending_set_and_material() {
    let names: Vec<&str> = Block::all()
        .iter()
        .filter_map(|b| crate::registry::names().blocks.name(b.id()))
        .filter(|n| faces_of(block_named(n).unwrap(), n).is_ok())
        .take(MAX_MATERIALS_PER_SET + 1)
        .collect();
    assert!(
        names.len() > MAX_MATERIALS_PER_SET,
        "the fixture needs enough opaque cubes"
    );
    let pairs: Vec<String> = names[1..]
        .iter()
        .enumerate()
        .map(|(i, n)| {
            format!(
                r#"{{"pair": "test:p{i}", "set": "petramond:organic", "blocks": ["{}", "{n}"]}}"#,
                names[0]
            )
        })
        .collect();
    let err = Rules::from_layers(&[&layer(&[ORGANIC], &pairs)]).unwrap_err();
    assert!(
        err.contains("petramond:organic") && err.contains(&format!("{MAX_MATERIALS_PER_SET}")),
        "{err}"
    );
    let sets: Vec<String> = (0..=MAX_SETS)
        .map(|i| {
            format!(
                r#"{{"set": "test:s{i:02}", "mask": "organic_transition_mask", "width_texels": 1}}"#
            )
        })
        .collect();
    let refs: Vec<&str> = sets.iter().map(String::as_str).collect();
    let err = Rules::from_layers(&[&layer(&refs, &[])]).unwrap_err();
    assert!(err.contains(&format!("{MAX_SETS}")), "{err}");
}

#[test]
fn the_shipped_catalog_loads_whole() {
    let r = rules();
    assert!(!r.sets.is_empty());
    assert!(r.sets.iter().all(|s| s.materials.len() >= 2));
}
