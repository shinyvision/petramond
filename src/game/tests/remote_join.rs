//! Contract tests for the remote client's entry point (multiplayer Phase E):
//! `Game::new_remote` seeds the whole client from `JoinData` alone (no save,
//! no `ServerGame`), and the roster tracks join/leave broadcasts.

use crate::game::Game;
use crate::item::ItemType;
use crate::mathh::{IVec3, Vec3};
use crate::net::protocol::{ItemSlotWire, JoinData, SelfRestore, ServerToClient};
use crate::server::handle::ServerHandle;
use crate::server::player::PlayerId;

fn join_data() -> Box<JoinData> {
    let mut slots: Vec<Option<ItemSlotWire>> = vec![None; 37];
    slots[2] = Some(ItemSlotWire {
        item_id: ItemType::Dirt.0,
        count: 12,
    });
    Box::new(JoinData {
        player_id: PlayerId(3),
        seed: 42,
        clock: 11_000,
        tables: crate::net::remap::local_name_tables(),
        self_restore: SelfRestore {
            pos: Vec3::new(4.5, 90.0, -7.5),
            vel: Vec3::ZERO,
            yaw: 1.5,
            pitch: -0.25,
            mode: 0,
            health: 13,
            bed_spawn: Some((IVec3::new(1, 70, 2), IVec3::new(2, 70, 2))),
            effects: vec![("petramond:regeneration".to_string(), 400)],
            inventory: slots,
            active_slot: 2,
        },
        players: vec![
            (PlayerId(0), "Host".to_string()),
            (PlayerId(1), "Visitor".to_string()),
        ],
    })
}

#[test]
fn new_remote_seeds_the_client_from_join_data() {
    let (handle, _pipe) = ServerHandle::loopback();
    let cam = crate::camera::Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0);
    let mut game = Game::new_remote(
        cam,
        join_data(),
        handle,
        1,
        "test-server",
        &std::collections::BTreeSet::new(),
    );

    // The locally-predicted player mirrors the restore (the wire twin of
    // `PlayerData::restore`).
    assert_eq!(game.player.pos, Vec3::new(4.5, 90.0, -7.5));
    assert_eq!(game.player.yaw, 1.5);
    assert_eq!(game.player.health(), 13);
    assert_eq!(
        game.player.bed_spawn.map(|b| b.bed),
        Some(IVec3::new(1, 70, 2))
    );
    assert_eq!(
        game.player.inventory.selected().map(|s| (s.item, s.count)),
        Some((ItemType::Dirt, 12)),
        "the restored active slot selects the restored stack"
    );
    assert_eq!(
        game.player
            .effects()
            .iter()
            .map(|e| (e.effect, e.remaining))
            .collect::<Vec<_>>(),
        vec![(crate::effect::Effect::Regeneration, 400)],
        "effects resolve by registry name"
    );
    // The HUD read model seeded from the same restore, before any batch.
    assert_eq!(game.self_view.health, 13);
    assert_eq!(
        game.self_view.inventory.selected().map(|s| s.item),
        Some(ItemType::Dirt)
    );
    assert_eq!(game.current_tick(), 0, "no tick replicated yet");

    // The join roster, then live join/leave broadcasts.
    assert_eq!(game.player_roster().len(), 2);
    assert_eq!(
        game.player_roster().get(&PlayerId(1)).map(String::as_str),
        Some("Visitor")
    );
    let mut msgs = vec![
        ServerToClient::PlayerJoined {
            id: PlayerId(2),
            name: "Guest".to_string(),
        },
        ServerToClient::PlayerLeft { id: PlayerId(1) },
    ];
    game.apply_server_messages(&mut msgs);
    assert_eq!(
        game.player_roster().get(&PlayerId(2)).map(String::as_str),
        Some("Guest")
    );
    assert!(!game.player_roster().contains_key(&PlayerId(1)));
}
