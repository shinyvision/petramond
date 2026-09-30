use super::*;
use crate::entity::DroppedItem;
use crate::events::Outcome;
use petramond_math::math::IVec3;
use petramond_math::world_pos::WorldPos;
use petramond_world::block::Block;
use petramond_world::item::{ItemStack, ItemType};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn fresh_server() -> ServerGame {
    crate::server::session_build::build_server_inline("", 1, 2)
}

fn block_impact(id: u64, cell: IVec3) -> ItemImpact {
    ItemImpact {
        id,
        target: ImpactTarget::Block {
            cell,
            face: IVec3::new(-1, 0, 0),
        },
        point: WorldPos::new(cell.x as f64, cell.y as f64 + 0.5, cell.z as f64 + 0.5),
        vel: Vec3::new(30.0, 0.0, 0.0),
    }
}

fn launch(server: &mut ServerGame, at: WorldPos) -> u64 {
    let it = DroppedItem::launched(
        at,
        ItemStack::new(ItemType::Stone, 1),
        Vec3::new(30.0, 0.0, 0.0),
        None,
    );
    server.world.spawn_item(it)
}

fn stone_cell(server: &mut ServerGame) -> IVec3 {
    let p = server.sessions[0].player.pos;
    let cell = IVec3::new(
        p.x.floor() as i32 + 2,
        p.y.floor() as i32 + 1,
        p.z.floor() as i32,
    );
    assert!(server
        .world
        .set_block_world(cell.x, cell.y, cell.z, Block::Stone));
    cell
}

#[test]
fn the_handlers_fate_is_applied_and_the_verdict_only_ends_the_dispatch() {
    let mut server = fresh_server();
    let cell = stone_cell(&mut server);
    let at = WorldPos::new(
        cell.x as f64 - 0.2,
        cell.y as f64 + 0.5,
        cell.z as f64 + 0.5,
    );
    let mut events = TickEvents::default();

    let id = launch(&mut server, at);
    server.resolve_item_impacts(vec![block_impact(id, cell)], &mut events);
    let it = server
        .world
        .dropped_items_mut()
        .get_mut(id)
        .expect("a drop stays");
    assert_eq!(it.motion, Motion::Loose, "stone does not stick");
    assert_eq!(it.vel, Vec3::ZERO, "a drop off a block stops dead");

    let seen = Arc::new(AtomicUsize::new(0));
    let first = seen.clone();
    server.mods.bus_mut().on_projectile_hit(0, move |_, ev| {
        first.fetch_add(1, Ordering::SeqCst);
        ev.fate = Fate::Lodge;
        Outcome::Cancel
    });
    let second = seen.clone();
    server.mods.bus_mut().on_projectile_hit(1, move |_, _| {
        second.fetch_add(10, Ordering::SeqCst);
        Outcome::Continue
    });
    let id = launch(&mut server, at);
    server.resolve_item_impacts(vec![block_impact(id, cell)], &mut events);
    assert_eq!(
        seen.load(Ordering::SeqCst),
        1,
        "Cancel ended the dispatch before the second handler"
    );
    let it = server
        .world
        .dropped_items_mut()
        .get_mut(id)
        .expect("lodged, not gone");
    assert!(matches!(it.motion, Motion::Stuck(s) if s.anchor == cell));

    let mut server = fresh_server();
    let cell = stone_cell(&mut server);
    server.mods.bus_mut().on_projectile_hit(0, |_, ev| {
        ev.fate = Fate::Consume;
        Outcome::Continue
    });
    let id = launch(&mut server, at);
    server.resolve_item_impacts(vec![block_impact(id, cell)], &mut events);
    assert!(
        server.world.dropped_items_mut().get_mut(id).is_none(),
        "consumed, though the handler said Continue"
    );
}

#[test]
fn an_impact_with_no_sessions_takes_the_default_fate() {
    let mut server = fresh_server();
    let cell = stone_cell(&mut server);
    let at = WorldPos::new(
        cell.x as f64 - 0.2,
        cell.y as f64 + 0.5,
        cell.z as f64 + 0.5,
    );
    let id = launch(&mut server, at);
    server.sessions.clear_for_test();
    let mut events = TickEvents::default();
    server.resolve_item_impacts(vec![block_impact(id, cell)], &mut events);
    let it = server
        .world
        .dropped_items_mut()
        .get_mut(id)
        .expect("dropped, not lost");
    assert_eq!(it.motion, Motion::Loose);
}

#[test]
fn a_lodge_on_a_body_drops_clear_instead() {
    let mut server = fresh_server();
    let at = server.sessions[0].player.pos + Vec3::new(2.0, 1.0, 0.0);
    let id = launch(&mut server, at);
    server.mods.bus_mut().on_projectile_hit(0, |_, ev| {
        ev.fate = Fate::Lodge;
        Outcome::Continue
    });
    let mut events = TickEvents::default();
    server.resolve_item_impacts(
        vec![ItemImpact {
            id,
            target: ImpactTarget::Mob(99),
            point: at,
            vel: Vec3::new(30.0, 0.0, 0.0),
        }],
        &mut events,
    );
    let it = server
        .world
        .dropped_items_mut()
        .get_mut(id)
        .expect("a drop");
    assert_eq!(it.motion, Motion::Loose);
    assert!(
        it.vel.x > 0.0 && it.vel.x < 30.0,
        "slowed, not stopped: {}",
        it.vel
    );
}

#[test]
fn a_deflection_keeps_the_projectile_and_reverses_its_flight() {
    let mut server = fresh_server();
    let at = server.sessions[0].player.pos + Vec3::new(2.0, 1.0, 0.0);
    let incoming = Vec3::new(30.0, -4.0, 5.0);
    let outgoing = incoming * -0.1;
    let owner = Some(EntityRef::Mob(99));
    let mut item = DroppedItem::launched(at, ItemStack::new(ItemType::Stone, 1), incoming, owner);
    if let Motion::Flight(flight) = &mut item.motion {
        flight.left_owner = true;
    }
    item.ticks_lived = 12;
    let id = server.world.spawn_item(item);
    server.mods.bus_mut().on_projectile_hit(0, move |_, ev| {
        ev.fate = Fate::Deflect { vel: outgoing };
        Outcome::Continue
    });
    server.resolve_item_impacts(
        vec![ItemImpact {
            id,
            target: ImpactTarget::Player(server.sessions[0].id()),
            point: at,
            vel: incoming,
        }],
        &mut TickEvents::default(),
    );
    let it = server.world.dropped_items_mut().get_mut(id).unwrap();
    let Motion::Flight(flight) = it.motion else {
        panic!("a deflection stays in flight");
    };
    assert_eq!(it.vel, outgoing);
    assert!(flight.heading.dir().dot(outgoing.normalize_or_zero()) > 0.9999);
    assert_eq!(flight.owner, owner);
    assert!(
        flight.left_owner,
        "the original launcher can still be struck"
    );
    assert_eq!(it.ticks_lived, 12);
    assert_eq!(it.stack, ItemStack::new(ItemType::Stone, 1));
    assert!(!it.collectable());
    let movement = it.advance_flight(0.05).expect("ordinary flight resumes");
    assert!(movement.dot(incoming) < 0.0);
    assert!(
        it.vel.y < outgoing.y,
        "gravity still acts on the bounced shot"
    );
}
