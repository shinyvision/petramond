use super::common::game;
use petramond::block::Block;
use petramond::item::ItemType;
use petramond::mathh::IVec3;

#[test]
fn stone_pickaxe_harvests_iron_as_raw_iron() {
    // Mining only spawns drops when harvested; the drop item comes from the
    // block's drop spec. Iron ore yields raw iron (here via spawn_drops, which
    // the mining path calls on a harvested break).
    let mut game = game();
    game.server.spawn_drops(
        IVec3::new(0, 64, 0),
        Block::IronOre,
        (15, petramond::light::BlockLight6::DARK),
        petramond::item::VariantId::NONE,
    );
    assert_eq!(game.server.world.item_entities().len(), 1);
    assert_eq!(
        game.server.world.item_entities()[0].stack.item,
        ItemType::RawIron
    );
}

#[test]
fn copper_ore_drops_two_to_four_raw_copper() {
    let mut game = game();
    game.server.spawn_drops(
        IVec3::new(1, 64, 1),
        Block::CopperOre,
        (15, petramond::light::BlockLight6::DARK),
        petramond::item::VariantId::NONE,
    );
    let drops = game.server.world.item_entities();
    assert_eq!(drops.len(), 1);
    assert_eq!(drops[0].stack.item, ItemType::RawCopper);
    assert!((2..=4).contains(&drops[0].stack.count), "2–4 raw copper");
}
