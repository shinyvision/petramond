use std::rc::Rc;

use super::*;
use crate::host::fake::rows::{BEDROCK, DIRT, STONE, TABLE};
use crate::host::fake::{Deed, Fake};

#[test]
fn the_packs_rows_resolve_and_every_row_carrying_the_data_is_scaffolding() {
    let world = Rc::new(Fake::new());
    // Bedrock carries the data too, but nothing places it: no scaffolding.
    world.state_mut().block_rows[usize::from(BEDROCK.0)]
        .data
        .push((SCAFFOLDING_DATA.into(), "true".into()));
    let _installed = world.install();
    let content = Content::resolve().expect("every row is registered");
    assert_eq!(content.table, TABLE);
    assert_eq!(content.golem, MobId(0));
    assert_eq!(content.blueprint, ItemId(10));
    assert_eq!(content.raw_copper, Some(ItemId(11)));
    let kinds: Vec<(&str, &str, f32, Option<&str>)> = content
        .scaffolding
        .iter()
        .map(|k| (k.name.as_str(), k.item.as_str(), k.hardness, k.tool.as_deref()))
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("petramond:dirt", "petramond:dirt", 0.5, Some("shovel")),
            ("petramond:oak_planks", "petramond:oak_planks", 2.0, Some("axe")),
        ]
    );
    assert_eq!(content.scaffold_kind(DIRT).map(|k| k.block), Some(DIRT));
    assert!(content.scaffold_kind(STONE).is_none());
    assert_eq!(content.scaffolding[1].record().block, "petramond:oak_planks");
}

#[test]
fn a_pack_missing_the_golem_leaves_the_mod_idle_and_says_why() {
    let world = Rc::new(Fake::new());
    world.state_mut().mob_rows.clear();
    let _installed = world.install();
    assert!(Content::resolve().is_none());
    assert!(world.deeds().contains(&Deed::Logged(format!(
        "mob '{GOLEM}' is not registered"
    ))));
}
