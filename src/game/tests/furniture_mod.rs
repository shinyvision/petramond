//! End-to-end coverage of the furniture pack's rideable bbmodel chair.
//!
//! Sitting is PURE furniture-mod policy over the actor-pose primitive
//! (`PlayerPoseSet` + roster-derived occupancy — the engine knows no block
//! seats); this smoke covers place / sit / sneak-dismount / break-release
//! through the real pack + WASM path.

use super::super::tick::TickEvents;
use crate::mathh::IVec3;

#[test]
fn chair_places_and_sits_via_wasm() {
    let Some(root) = crate::modding::tests::stage_mods_fixture("furniture", &["furniture"]) else {
        return; // the pack isn't built (`make mods`); skip rather than fail.
    };
    crate::modding::tests::run_child_test(&root, "game::tests::furniture_mod::chair_inner");
}

#[test]
#[ignore = "spawned with the furniture fixture before registry initialization"]
fn chair_inner() {
    use crate::block::{Block, ShapeFamily};
    use crate::facing::Facing;
    use crate::item::ItemStack;

    let mut game = super::common::game_on_empty_chunk();
    game.server.sessions[0].intent_gameplay = true;
    for x in 0..16 {
        for z in 0..16 {
            assert!(game.server.world.set_block_world(x, 63, z, Block::Stone));
        }
    }

    let chair_id = crate::registry::names()
        .blocks
        .id("furniture:chair")
        .expect("furniture:chair registered from the fixture pack");
    let chair = Block::from_id(chair_id);
    assert_eq!(chair.shape_family(), ShapeFamily::Model);
    assert!(chair.model_kind().is_some());

    let item = crate::registry::names()
        .items
        .id("furniture:chair")
        .expect("furniture:chair item registered");
    let recipe = game
        .server
        .recipes
        .crafting()
        .get_at(
            "furniture:chair",
            crate::crafting::CraftingStation::FurnitureWorkbench,
        )
        .expect("the chair is registered at the furniture workbench");
    assert_eq!(
        recipe.result().item,
        crate::item::ItemType(item),
        "the recipe yields a chair"
    );

    let pos = IVec3::new(7, 64, 7);
    assert!(
        game.server
            .world
            .place_model_block_facing(pos, chair, Facing::South),
        "the chair places as a model block"
    );
    assert_eq!(
        game.server.world.chunk_block(pos.x, pos.y, pos.z),
        chair_id
    );
    // The backrest cell is REAL occupancy ([1,2,1]): the chair owns the seated
    // body's headroom, so nothing can ever be placed into it.
    assert_eq!(
        game.server.world.chunk_block(pos.x, pos.y + 1, pos.z),
        chair_id,
        "the chair occupies its backrest cell"
    );

    // Use-click the chair: furniture WASM seats the player.
    let player_id = game.server.sessions[0].id.0;
    game.server.sessions[0].player.pos = crate::mathh::Vec3::new(7.5, 64.0, 5.5);
    game.server.sessions[0].look = Some(super::common::hit(pos, IVec3::Y));
    game.server.queue_place_click_for_test(0);
    let mut events = TickEvents::default();
    game.server.tick_place(0, &mut events);
    // Riding pass runs in the Mobs stage of a full tick step.
    game.server.game_tick_step(&mut events);

    let mount = game
        .server
        .world
        .riding()
        .mount_of(player_id)
        .expect("the player is seated on the chair");
    let crate::mob::riding::MountTarget::Anchor(anchor) = mount.target else {
        panic!("a chair sit is a pose anchor, got {:?}", mount.target);
    };
    assert!(anchor.pos.is_finite());
    // The mod computed the anchor from ITS seat layout inside the chair's
    // base cell footprint (exact offsets are pack balance, not pinned here).
    assert!(
        anchor.pos.x > pos.x as f32
            && anchor.pos.x < (pos.x + 1) as f32
            && anchor.pos.z > pos.z as f32
            && anchor.pos.z < (pos.z + 1) as f32
            && (anchor.pos.y - pos.y as f32).abs() < 1.0,
        "the seat anchor lands inside the chair cell, got {:?}",
        anchor.pos
    );
    assert!(
        (game.server.sessions[0].player.pos - anchor.pos).length() < 1e-3,
        "the rider is slaved to the pose anchor"
    );

    // Sneak rising edge dismounts.
    game.server.sessions[0].intent_sneak = true;
    game.server.game_tick_step(&mut events);
    assert!(
        game.server.world.riding().mount_of(player_id).is_none(),
        "sneak dismounts from the chair"
    );
    game.server.sessions[0].intent_sneak = false;

    // Clicking the BACKREST cell seats too: any group cell resolves to the
    // group base for the mount.
    let upper = pos + IVec3::Y;
    game.server.sessions[0].look = Some(super::common::hit(upper, IVec3::new(0, 0, -1)));
    game.server.queue_place_click_for_test(0);
    game.server.tick_place(0, &mut events);
    game.server.game_tick_step(&mut events);
    let mount = game
        .server
        .world
        .riding()
        .mount_of(player_id)
        .expect("a backrest click seats the player");
    let crate::mob::riding::MountTarget::Anchor(back_anchor) = mount.target else {
        panic!("a backrest sit is a pose anchor, got {:?}", mount.target);
    };
    assert_eq!(
        back_anchor.pos, anchor.pos,
        "any group cell resolves to the same seat anchor"
    );
    game.server.sessions[0].intent_sneak = true;
    game.server.game_tick_step(&mut events);
    game.server.sessions[0].intent_sneak = false;

    // Sit again, then BREAK the chair through the real player-break path —
    // the pose is not tied to the block, so this exercises the mod's
    // `block_broken` release (hypothesis search from the broken cell).
    game.server.sessions[0].look = Some(super::common::hit(pos, IVec3::Y));
    game.server.queue_place_click_for_test(0);
    game.server.tick_place(0, &mut events);
    game.server.game_tick_step(&mut events);
    assert!(game.server.world.riding().mount_of(player_id).is_some());
    game.server.finish_player_break(
        0,
        crate::mining::BreakEvent {
            pos: pos + IVec3::Y, // the aimed backrest cell, not the base
            block: chair,
            harvested: true,
        },
        &mut events,
        true,
    );
    assert_eq!(
        game.server.world.chunk_block(pos.x, pos.y, pos.z),
        Block::Air.0,
        "breaking any cell removes the whole group"
    );
    game.server.game_tick_step(&mut events);
    assert!(
        game.server.world.riding().mount_of(player_id).is_none(),
        "the furniture mod releases the sitter when its chair breaks"
    );

    // Placement path still works from the held item.
    game.server.sessions[0]
        .player
        .inventory
        .add(ItemStack::new(crate::item::ItemType(item), 1));
    let floor = IVec3::new(9, 63, 9);
    game.server.sessions[0].look = Some(super::common::hit(floor, IVec3::Y));
    game.server.queue_place_click_for_test(0);
    game.server.game_tick_step(&mut events);
    let placed = floor + IVec3::Y;
    assert_eq!(
        game.server.world.chunk_block(placed.x, placed.y, placed.z),
        chair_id,
        "the chair places from the held item"
    );

    // Client prediction: the furniture CLIENT instance mirrors the sit gate
    // against the replica, so a click on any chair cell while holding a
    // placeable block predicts the mod claim (no place ghost).
    use crate::game::tick::PlacePrediction;
    game.server.sessions[0]
        .player
        .inventory
        .add(ItemStack::new(crate::item::ItemType(item), 1));
    game.sync_self_view_for_test();
    game.game.self_view.inventory.set_active(0);
    assert_eq!(
        game.game
            .self_view
            .inventory
            .selected()
            .map(|s| s.item.0),
        Some(item),
        "the client holds the chair item"
    );
    super::common::flat_floor_loaded_air(&mut game.game.replica, Block::Stone);
    game.game.replica.set_block_world(pos.x, pos.y, pos.z, chair);
    game.game
        .replica
        .set_block_world(pos.x, pos.y + 1, pos.z, chair);
    assert!(
        matches!(
            game.game.predict_place_at_for_test(pos, IVec3::X, false),
            PlacePrediction::No
        ),
        "the predicted sit claim suppresses the place ghost"
    );
    assert!(
        matches!(
            game.game.predict_place_at_for_test(pos + IVec3::Y, IVec3::X, false),
            PlacePrediction::No
        ),
        "a backrest-cell click predicts the sit claim too"
    );
}
