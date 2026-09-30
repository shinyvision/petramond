use super::game::ServerGame;
use crate::entity::{Fate, Motion};
use crate::events::tick::TickEvents;
use crate::events::ProjectileHit;
use crate::mob::EntityRef;
use crate::world::{ImpactTarget, ItemImpact};
use petramond_math::math::Vec3;

const BODY_DROP_SPEED: f32 = 0.1;

impl ServerGame {
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
        mods.bus_mut()
            .projectile_hit(world, sessions, actor, events, ev);
    }

    fn apply_fate(&mut self, impact: ItemImpact, fate: Fate) {
        let drops = self.world.dropped_items_mut();
        if fate == Fate::Consume {
            drops.remove(impact.id);
            return;
        }
        let Some(it) = drops.get_mut(impact.id) else {
            return;
        };
        if let Fate::Deflect { vel } = fate {
            if vel.x.is_finite() && vel.y.is_finite() && vel.z.is_finite() {
                it.deflect(vel);
                return;
            }
            log::warn!("projectile_hit: non-finite deflection velocity — dropping instead");
        }
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
mod tests;
