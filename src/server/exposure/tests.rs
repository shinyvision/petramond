use super::*;
use crate::mob::{Mob, PlayerAnchor};
use petramond_math::math::Vec3;
use petramond_world::block::Block;
use petramond_world::condition::ConditionId;
use petramond_world::fluid::FluidDef;

/// A shipped fluid whose contact both deals damage and applies a condition.
/// The wiring under test is generic; which row supplies it is not.
fn hazard() -> &'static FluidDef {
    Block::all()
        .iter()
        .filter_map(|b| b.fluid_def())
        .find(|f| f.contact.damage.is_some() && f.contact.applies.is_some())
        .expect("a shipped fluid deals contact damage and applies a condition")
}

fn contact_source() -> DamageSource {
    DamageSource::Fluid(hazard().block)
}

fn condition() -> ConditionId {
    hazard().contact.applies.unwrap().condition
}

fn condition_source() -> DamageSource {
    DamageSource::Condition(condition())
}

fn contact_interval() -> u32 {
    hazard().contact.damage.unwrap().interval
}

/// The longest pulse interval any stage of the applied condition runs.
fn pulse_interval() -> u32 {
    condition()
        .def()
        .stages
        .iter()
        .filter_map(|s| s.damage.map(|d| d.interval))
        .max()
        .expect("the applied condition pulses damage")
}

fn has_condition(player: &crate::player::Player) -> bool {
    player.conditions().get(condition()).is_some()
}

fn server() -> ServerGame {
    let mut server = crate::server::session_build::build_server_inline("", 1, 1);
    for y in 64..69 {
        server
            .world
            .set_block_world(8, y, 8, if y == 64 { Block::Stone } else { Block::Air });
    }
    server.sessions[0].player.pos = Vec3::new(8.5, 65.0, 8.5);
    server
}

fn record_player_damage(
    server: &mut ServerGame,
) -> std::sync::Arc<std::sync::Mutex<Vec<DamageSource>>> {
    let hits = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = hits.clone();
    server.bus.on_player_damage_pre(0, move |_, hit| {
        sink.lock().unwrap().push(hit.source);
        crate::events::Outcome::Continue
    });
    hits
}

#[test]
fn player_conditions_replicate_and_damage_sources_remain_distinct() {
    let mut server = server();
    let hits = record_player_damage(&mut server);
    let mut events = TickEvents::default();
    server.damage_player(0, 1, DamageSource::Fall, None, &mut events);
    assert!(server.world.set_block_world(8, 65, 8, hazard().block));
    server.tick_player_exposure(0, &mut events);
    assert!(
        hits.lock().unwrap().contains(&contact_source()),
        "contact damage lands during combat immunity"
    );
    assert!(server.sessions[0].player.effects().is_empty());
    assert!(!server.build_self_state(0).conditions.is_empty());
    assert!(!replicated(&mut server, &events).players[0]
        .conditions
        .is_empty());

    server.world.set_block_world(8, 65, 8, Block::Air);
    for _ in 0..pulse_interval() {
        server.tick_player_exposure(0, &mut events);
    }
    let hits = hits.lock().unwrap();
    assert!(hits.contains(&condition_source()));
    assert_eq!(hits.iter().filter(|s| **s == contact_source()).count(), 1);
}

#[test]
fn swimming_keeps_contact_damage_and_its_condition_active() {
    let mut server = server();
    for y in 65..=69 {
        assert!(server.world.set_block_world(8, y, 8, hazard().block));
    }
    server.sessions[0].move_jump = true;
    server.sessions[0].intent_gameplay = true;
    let health = server.sessions[0].player.health();
    let hits = record_player_damage(&mut server);
    for _ in 0..=pulse_interval().max(contact_interval()) {
        server.tick_movement(0);
        server.tick_player_exposure(0, &mut TickEvents::default());
    }
    let player = &server.sessions[0].player;
    assert!(!player.on_ground);
    assert!(player.health() < health);
    assert!(has_condition(player));
    assert_eq!(server.sessions[0].pending_fall, 0.0);
    let hits = hits.lock().unwrap();
    assert!(hits.contains(&contact_source()));
    assert!(hits.contains(&condition_source()));
}

#[test]
fn cancelled_damage_does_not_cancel_a_condition_or_change_its_cadence() {
    let mut server = server();
    server
        .bus
        .on_player_damage_pre(0, |_, _| crate::events::Outcome::Cancel);
    server.world.set_block_world(8, 65, 8, hazard().block);
    let health = server.sessions[0].player.health();
    for _ in 0..contact_interval() * 3 {
        server.tick_player_exposure(0, &mut TickEvents::default());
    }
    assert_eq!(server.sessions[0].player.health(), health);
    assert!(has_condition(&server.sessions[0].player));
    server.sessions[0]
        .player
        .set_mode(crate::player::PlayerMode::Spectator);
    server.tick_player_exposure(0, &mut TickEvents::default());
    assert!(!has_condition(&server.sessions[0].player));
}

fn mob_tick(server: &mut ServerGame) {
    server
        .world
        .mobs_mut()
        .set_mob_kinematic(
            0,
            Vec3::new(8.5, 65.0, 8.5),
            0.0,
            petramond_math::math::Tilt::LEVEL,
        )
        .unwrap();
    let events = server.world.tick_mobs(
        crate::events::tick::TICK_DT,
        &[PlayerAnchor {
            pos: Vec3::new(12.0, 65.0, 12.0),
            ..Default::default()
        }],
    );
    server.apply_mob_exposure_damage(events.exposure, &mut TickEvents::default());
}

#[test]
fn a_mob_keeps_a_contact_condition_and_its_emitter_after_leaving_the_fluid() {
    let mut server = server();
    server
        .world
        .mobs_mut()
        .spawn(Mob::Sheep, Vec3::new(8.5, 65.0, 8.5), 0.0);
    let initial = server.world.mobs().instances()[0].health();
    server.world.set_block_world(8, 65, 8, hazard().block);
    mob_tick(&mut server);
    let contact_health = server.world.mobs().instances()[0].health();
    assert!(contact_health < initial);
    let active = server.world.mobs().instances()[0]
        .exposure()
        .conditions()
        .get(condition())
        .unwrap()
        .clone();
    assert!(replicated(&mut server, &TickEvents::default()).mobs[0]
        .conditions
        .contains(&(condition().0, active.stage())));
    server.world.set_block_world(8, 65, 8, Block::Air);
    for _ in 0..pulse_interval() {
        mob_tick(&mut server);
    }
    assert!(server.world.mobs().instances()[0].health() < contact_health);
}

#[test]
fn touched_fluids_use_body_edges_and_actual_flow_height() {
    let mut server = server();
    let fluid = hazard().block;
    server.world.set_block_world(8, 65, 8, fluid);
    server
        .world
        .section_at_world_mut_for_test(8, 65, 8)
        .unwrap()
        .set_fluid(8, 1, 8, fluid, 7);
    let body = |x, y| petramond_world::body::Body::new(Vec3::new(x, y, 8.5), 0.3, 1.8).aabb();
    let touched = |x, y| {
        crate::exposure::touched_fluids(&server.world, [body(x, y)])
            .map(|fluids| fluids.iter().map(|f| f.block).collect::<Vec<_>>())
    };
    assert_eq!(touched(8.5, 65.5), Some(Vec::new()));
    assert_eq!(
        touched(9.1, 65.0),
        Some(vec![fluid]),
        "a shoulder can touch the fluid while the centre is outside"
    );
}

fn replicated(server: &mut ServerGame, events: &TickEvents) -> crate::net::protocol::TickUpdate {
    let shared = server.shared_tick_rows(events);
    server.build_tick_update(0, events, &[], &[], &[], &[], &shared)
}
