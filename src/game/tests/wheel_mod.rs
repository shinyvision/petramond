//! Full-loop proof for the wheel PoC mod: a mod block placed in the world, a
//! mod GUI session, a widget click dispatched to wasm, a tick-animated spin,
//! and EXACTLY ONE of the five rewards materializing through the host API
//! (give_item / spawn_mob / kill_player). Which reward is seed-dependent and
//! deliberately unpinned. Pack registration needs the fixture in the
//! registry, so the assertions run in a child process (the established
//! `PETRAMOND_MODS` re-spawn pattern, staged by `modding::tests`).

use super::super::tick::TickEvents;
use crate::camera::Camera;
use crate::mathh::Vec3;

#[test]
fn wheel_mod_spin_delivers_exactly_one_reward_via_wasm() {
    let Some(root) = crate::modding::tests::stage_mods_fixture("wheel-spin", &["wheel"]) else {
        return;
    };
    crate::modding::tests::run_child_test(&root, "game::tests::wheel_mod::wheel_spin_inner");
}

/// Runs ONLY in the child process spawned above (needs `PETRAMOND_MODS`
/// pointing at the fixture pack before first registry touch). Production load
/// path: `Game::new` → `ModHost::load`, clicks through the real menu funnel.
#[test]
#[ignore = "spawned by wheel_mod_spin_delivers_exactly_one_reward_via_wasm with a fixture pack env"]
fn wheel_spin_inner() {
    use crate::block::Block;
    use crate::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};
    use crate::controls::PointerButton;
    use crate::gui::{GuiValue, MenuSlot};
    use crate::item::ItemType;
    use crate::mathh::IVec3;

    let wheel_item = ItemType::all()
        .iter()
        .copied()
        .find(|i| i.key() == "wheel:wheel_of_fortune")
        .expect("wheel item registered from the fixture pack");
    let wheel_block = wheel_item
        .as_block()
        .expect("the wheel item links to its block");

    let mut game =
        super::common::game_with_camera(Camera::new(Vec3::new(8.0, 66.0, 8.0), 16.0 / 9.0));
    assert_eq!(game.mods_for_test().loaded(), 1, "the wheel wasm loaded");
    // A STONE platform: no mob species natural-spawns on stone, so a sheep
    // party is countable. Empty columns first (the fixture gotcha: the air
    // above the floor must read as loaded for the mod's ground scans).
    game.server.world.clear_world();
    let pos = ChunkPos::new(0, 0);
    game.server.world.insert_empty_column_for_test(pos);
    let mut chunk = Chunk::new(0, 0);
    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            chunk.set_block(x, 63, z, Block::Stone);
        }
    }
    game.server.world.insert_chunk_for_test(pos, chunk);
    game.server.sessions[0].player.pos = Vec3::new(8.0, 64.0, 8.0);
    game.server.sessions[0].player.vel = Vec3::ZERO;
    game.server.sessions[0].player.on_ground = true;

    // The mod block goes into the world through the engine edit API, then the
    // GUI session opens from it (the interaction row's own parse → GuiKind
    // registration is what `resolve_kind` finds here).
    let block_pos = IVec3::new(12, 64, 8);
    assert!(game.server.world.set_block_world(12, 64, 8, wheel_block));
    let kind = crate::gui::resolve_kind("wheel:wheel")
        .expect("the pack's open_gui interaction registered the kind");
    game.server
        .open_mod_gui_screen_for(0, kind, Some(block_pos));

    let count = |game: &super::common::TestGame, item: ItemType| -> u32 {
        (0..crate::inventory::TOTAL_SLOTS)
            .filter_map(|i| game.server.sessions[0].player.inventory.slot(i))
            .filter(|s| s.item == item)
            .map(|s| s.count as u32)
            .sum()
    };
    let sheep = |game: &super::common::TestGame| {
        game.server
            .world
            .mobs()
            .instances()
            .iter()
            .filter(|m| m.kind == crate::mob::Mob::Sheep)
            .count()
    };
    assert_eq!(sheep(&game), 0, "no mobs before the spin");
    let h0 = game.server.sessions[0].player.health();

    // Click spin; a second click mid-spin must be ignored (still exactly one
    // reward at the end).
    game.menu_click(
        MenuSlot::Widget("spin"),
        PointerButton::Primary,
        false,
        false,
    );
    game.flush_outbox_for_test();
    let mut ev = TickEvents::default();
    for tick in 0..70 {
        game.server.game_tick_step(&mut ev);
        if tick == 10 {
            match game.server.sessions[0].gui_state.get("wheel:result") {
                None | Some(GuiValue::Str(_)) => {} // empty or cleared mid-spin
                other => panic!("unexpected mid-spin result value: {other:?}"),
            }
            if let Some(GuiValue::Str(s)) = game.server.sessions[0].gui_state.get("wheel:result") {
                assert!(s.is_empty(), "no announcement before the wheel lands");
            }
            game.menu_click(
                MenuSlot::Widget("spin"),
                PointerButton::Primary,
                false,
                false,
            );
            game.flush_outbox_for_test();
        }
    }

    // The session stayed open: the announcement is visible and the wheel
    // rests EXACTLY on a segment centre (angle ≡ -k·72° mod 360°).
    match game.server.sessions[0].gui_state.get("wheel:result") {
        Some(GuiValue::Str(s)) if !s.is_empty() => {}
        other => panic!("wheel:result should hold the announcement, got {other:?}"),
    }
    match game.server.sessions[0].gui_state.get("wheel:angle") {
        Some(GuiValue::F32(a)) => {
            let slots = a * 5.0 / core::f32::consts::TAU;
            assert!(
                (slots - slots.round()).abs() < 1e-3,
                "the wheel landed exactly on a segment grid angle, got {a}"
            );
        }
        other => panic!("wheel:angle should be live, got {other:?}"),
    }

    // EXACTLY one of the five outcomes materialized (which one is
    // seed-dependent; the ignored mid-spin click must not double-deliver).
    let outcomes = [
        count(&game, ItemType::Diamond) == 1,
        count(&game, ItemType::Stick) == 1,
        count(&game, ItemType::Coal) == 1,
        sheep(&game) == 5,
        game.server.sessions[0].player.health() == 0,
    ];
    assert_eq!(
        outcomes.iter().filter(|&&o| o).count(),
        1,
        "exactly one reward materialized (h0 {h0}, health {}, outcomes {outcomes:?})",
        game.server.sessions[0].player.health()
    );
    let (disabled, _, _) = game.mods_for_test().probe(0);
    assert!(!disabled, "the wheel mod stayed healthy through the spin");
}
