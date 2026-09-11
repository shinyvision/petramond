use super::*;

#[test]
fn shipped_loot_tables_resolve_and_have_bounded_expansion() {
    let _ = catalog();
}

fn parse(text: &str) -> Result<Loot, String> {
    parse_layers(
        &[include_str!("../../../../assets/loot_tables.json"), text],
        |_| {
            crate::registry::names()
                .items
                .id("petramond:stick")
                .map(ItemType)
        },
    )
}

#[test]
fn weighted_nested_rewards_use_only_the_supplied_random_stream() {
    let loot = parse(
        r#"{"loot_tables":[
      {"loot":"fixture:root","pools":[{"rolls":[1,1],"entries":[
        {"weight":1,"result":{"type":"empty"}},
        {"weight":2,"result":{"type":"table","table":"fixture:child"}}
      ]}]},
      {"loot":"fixture:child","pools":[{"rolls":[2,2],"entries":[
        {"weight":1,"result":{"type":"item","item":"fixture:stick","count":[1,3]}}
      ]}]}
    ]}"#,
    )
    .unwrap();
    assert!(loot.roll("fixture:root", || 0).unwrap().is_empty());
    let stacks = loot.roll("fixture:root", || 1).unwrap();
    assert_eq!(stacks.len(), 2);
    assert!(stacks.iter().all(|s| s.count == 2));
    assert_eq!(stacks, loot.roll("fixture:root", || 1).unwrap());
    assert!(loot
        .roll("fixture:missing", || panic!("unknown tables draw nothing"))
        .is_none());
}

#[test]
fn cycles_and_multiplicative_expansion_fail_before_any_roll() {
    for child in ["fixture:a", "fixture:b"] {
        let text = format!(
            r#"{{"loot_tables":[
          {{"loot":"fixture:a","pools":[{{"rolls":[32,32],"entries":[
            {{"weight":1,"result":{{"type":"table","table":"{child}"}}}}
          ]}}]}},
          {{"loot":"fixture:b","pools":[{{"rolls":[32,32],"entries":[
            {{"weight":1,"result":{{"type":"empty"}}}}
          ]}}]}}
        ]}}"#
        );
        assert!(parse(&text).is_err());
    }
}

#[test]
fn invalid_counts_weights_and_unknown_references_are_rejected() {
    for entry in [
        r#"{"weight":0,"result":{"type":"empty"}}"#,
        r#"{"weight":1,"result":{"type":"table","table":"fixture:missing"}}"#,
        r#"{"weight":1,"result":{"type":"item","item":"fixture:x","count":[0,1]}}"#,
        r#"{"weight":1,"result":{"type":"item","item":"fixture:x","count":[2,1]}}"#,
        r#"{"weight":1,"result":{"type":"item","item":"fixture:x","count":[1,255]}}"#,
    ] {
        let text = format!(
            r#"{{"loot_tables":[{{"loot":"fixture:a","pools":[{{"rolls":[1,1],"entries":[{entry}]}}]}}]}}"#
        );
        assert!(parse(&text).is_err());
    }
}
