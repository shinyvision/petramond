//! Resolving a flying item's impact: the world store found what it struck
//! (`ItemImpact`); this is where the strike becomes a consequence — a
//! `projectile_hit` dispatch carrying the engine's default FATE (lodge in
//! the block when the row `sticks`, else drop loose at the impact) for the
//! handlers to rewrite, then that fate applied. The engine deals no damage
//! here; what a launched item does to what it hits is the launching mod's
//! law, landed through the ordinary damage calls from a handler.

use super::game::ServerGame;
use crate::entity::{Fate, Motion};
use crate::events::tick::TickEvents;
use crate::events::ProjectileHit;
use crate::mob::EntityRef;
use crate::world::{ImpactTarget, ItemImpact};
use petramond_math::math::Vec3;

/// How much of its arrival speed a flying item that struck a BODY keeps as
/// it drops loose: enough to fall clear of the body, not enough to read as
/// a bounce.
const BODY_DROP_SPEED: f32 = 0.1;

impl ServerGame {
    /// Resolve every impact this tick's item physics reported, in order.
    pub fn resolve_item_impacts(&mut self, impacts: Vec<ItemImpact>, events: &mut TickEvents) {
        for impact in impacts {
            self.resolve_item_impact(impact, events);
        }
    }

    fn resolve_item_impact(&mut self, impact: ItemImpact, events: &mut TickEvents) {
        let Some((item, sticks, owner)) = self.world.dropped_items().get(impact.id).map(|it| {
            let owner = match it.motion {
                Motion::Flight(f) => f.owner,
                _ => None,
            };
            (it.stack.item, it.stack.item.projectile().sticks, owner)
        }) else {
            return;
        };
        let struck_block = matches!(impact.target, ImpactTarget::Block { .. });
        let mut ev = ProjectileHit {
            entity: impact.id,
            item,
            owner,
            target: impact.target,
            pos: impact.point,
            vel: impact.vel,
            fate: Fate::of_impact(sticks, struck_block),
        };
        self.dispatch_projectile_hit(&mut ev, events);
        self.apply_fate(impact, ev.fate);
    }

    /// Run the `projectile_hit` handlers over `ev`. The dispatch acts for
    /// the LAUNCHER when it is a connected player — so a handler's
    /// `PlayerState`, and the damage it lands naming the presser, resolve the
    /// launcher. Any other launcher (a mob's, or a player gone) dispatches
    /// actor-less: a handler addresses by the payload's ids (`owner`,
    /// `target`, `entity`).
    fn dispatch_projectile_hit(&mut self, ev: &mut ProjectileHit, events: &mut TickEvents) {
        let actor = ev
            .owner
            .and_then(EntityRef::player)
            .filter(|&id| self.sessions.by_id(id).is_some());
        let Self {
            world,
            sessions,
            mods,
            ..
        } = self;
        mods.bus_mut().projectile_hit(world, sessions, actor, events, ev);
    }

    /// The settled fate, on the entity: a lodge needs the block it struck,
    /// a drop off a body keeps a whisper of its speed so it falls clear, a
    /// drop off a block stops dead. The engine has no body-attached motion,
    /// so a handler asking to lodge in a BODY is a mod bug: logged, and
    /// applied as the drop the default already was.
    fn apply_fate(&mut self, impact: ItemImpact, fate: Fate) {
        let drops = self.world.dropped_items_mut();
        if fate == Fate::Consume {
            drops.remove(impact.id);
            return;
        }
        let Some(it) = drops.get_mut(impact.id) else {
            return;
        };
        match (fate, impact.target) {
            (Fate::Lodge, ImpactTarget::Block { cell, .. }) => it.lodge(cell),
            (_, ImpactTarget::Block { .. }) => {
                it.vel = Vec3::ZERO;
                it.release();
            }
            (fate, ImpactTarget::Mob(_) | ImpactTarget::Player(_)) => {
                if fate == Fate::Lodge {
                    log::warn!(
                        "projectile_hit: a handler answered Lodge for {} striking {:?}; \
                         only a block can hold an item (mod bug) — dropping instead",
                        it.stack.item.key(),
                        impact.target
                    );
                }
                it.vel *= BODY_DROP_SPEED;
                it.release();
            }
        }
    }
}

#[cfg(test)]
mod tests {
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

    /// An impact against a stone cell just ahead of the item.
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

    /// A cell the test world is sure to hold: beside the listen player.
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

    /// The default fate is the engine's own — a row that does not stick
    /// drops loose, dead, at a block — and a handler's rewrite is what is
    /// applied: `Lodge` seats the item in the cell, `Consume` removes it.
    /// The verdict never decides the fate.
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

        // A fresh bus: a Consume from a handler that Continues is applied
        // exactly like a Cancelling one's.
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

    /// An impact with nobody connected dispatches actor-less over an empty
    /// roster; with no handler to dispute it, the engine's default stands.
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

    /// A lodge is a block's to hold: asked for on a body, it is a drop that
    /// keeps a whisper of speed to fall clear (and a logged mod bug).
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
}
