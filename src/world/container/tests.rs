use petramond_math::math::IVec3;
use petramond_world::block::Block;

use super::LOOT_KEY;
use crate::world::testutil::flat_server_world;

#[test]
fn a_loot_marker_stocks_its_container_once() {
    let mut world = flat_server_world();
    let pos = IVec3::new(3, 70, 3);
    let marker = br#"{"table": "petramond:owl_drops"}"#.to_vec();
    let rolled = (0..64).any(|attempt| {
        let at = IVec3::new(pos.x + attempt % 8, pos.y, pos.z + attempt / 8);
        world.set_block_world(at.x, at.y, at.z, Block::Chest);
        assert!(world.cell_kv_set(at.x, at.y, at.z, LOOT_KEY.into(), marker.clone()));
        world.stock_loot(at);
        assert!(
            world.data.cell_kv_get(at.x, at.y, at.z, LOOT_KEY).is_none(),
            "the marker is consumed"
        );
        let first: Vec<_> = world.container_at(at).unwrap().slots.clone();
        world.stock_loot(at);
        assert_eq!(
            world.container_at(at).unwrap().slots,
            first,
            "a looted container never refills"
        );
        first.iter().any(Option::is_some)
    });
    assert!(rolled, "some chest out of 64 rolled a non-empty owl table");
}
