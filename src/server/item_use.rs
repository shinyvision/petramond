use super::game::ServerGame;
use crate::entity::DroppedItem;
use crate::events::tick::TickEvents;
use crate::events::{BlockPlacePre, ItemUseEvent, ItemUsePre, Outcome, PostEvent};
use crate::mob::ShearDrop;
use crate::net::protocol::TargetRef;
use crate::rules::item_use::{self as rules, EngineItemUse};
use crate::rules::placement::facing_from_forward;
use petramond_math::math::{IVec3, Vec3};
use petramond_world::block::Block;
use petramond_world::item::{ItemStack, ItemType};
use petramond_world::world::raycast;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct EatingState {
    pub hand: petramond_world::inventory::Hand,
    pub slot: u8,
    pub item: ItemType,
    pub progress: u32,
}

impl ServerGame {
    pub fn authoritative_use_target(
        &self,
        s: usize,
        held_item: Option<ItemType>,
        claimed: Option<TargetRef>,
    ) -> Option<TargetRef> {
        let sess = &self.sessions[s];
        let eye = super::movement::reach_eye(sess);
        let claimed = claimed.filter(|target| crate::player::block_within_reach(eye, target.block));
        let Some(ray) = held_item
            .map(|item| item.use_ray())
            .filter(|ray| ray.sees_fluid())
        else {
            return claimed;
        };

        let authoritative = raycast::use_ray(eye, sess.player.forward(), self.world.data(), ray)
            .map(|(hit, _)| (hit.block, hit.normal));
        if claimed.map(|c| (c.block, c.normal)) == authoritative {
            claimed
        } else {
            None
        }
    }

    pub fn try_start_eating(&mut self, s: usize, events: &mut TickEvents) -> bool {
        let sess = &self.sessions[s];
        let hand = sess.player.acting_hand;
        if !rules::eat_claims(&sess.player) {
            return false;
        }
        let Some(item) = rules::held_item(&sess.player) else {
            return false;
        };
        let slot = sess.player.inventory.active_slot();
        if sess.sim.eating.is_some_and(|e| {
            e.hand == hand
                && e.item == item
                && (hand == petramond_world::inventory::Hand::Off || e.slot == slot)
        }) {
            return true;
        }
        let target = sess.input.look.map(|h| h.block);
        let mut pre = ItemUsePre { item, target };
        let cancelled = {
            let Self {
                world,
                sessions,
                mods,
                ..
            } = self;
            let actor = Some(sessions[s].id);
            let bus = mods.bus_mut();
            bus.item_use_pre(world, sessions, actor, events, &mut pre) == Outcome::Cancel
        };
        if cancelled {
            self.mods.emit(PostEvent::ItemUsed {
                player: self.sessions[s].id,
                item,
                kind: ItemUseEvent::Claimed,
            });
            return true;
        }
        self.sessions[s].sim.eating = Some(EatingState {
            hand,
            slot,
            item,
            progress: 0,
        });
        true
    }

    pub fn advance_eating(&mut self, s: usize) {
        let sess = &mut self.sessions[s];
        let Some(eat) = sess.sim.eating else {
            return;
        };
        let held = sess.player.inventory.held_in(eat.hand).map(|st| st.item);
        let selection_moved = eat.hand == petramond_world::inventory::Hand::Main
            && sess.player.inventory.active_slot() != eat.slot;
        let barred = sess
            .player
            .denied_actions()
            .denies(mod_api::BodyAction::Use);
        if !sess.input.intent_use_held || barred || selection_moved || held != Some(eat.item) {
            sess.sim.eating = None;
            if sess
                .player
                .use_gesture
                .held_by(crate::player::ENGINE_CLAIMANT)
            {
                sess.player.use_gesture = crate::player::UseGesture::Free;
            }
            return;
        }
        let Some(food) = eat.item.food() else {
            sess.sim.eating = None;
            return;
        };
        let progress = eat.progress + 1;
        if progress < food.eat_ticks {
            sess.sim.eating = Some(EatingState { progress, ..eat });
            return;
        }
        sess.sim.eating = None;
        sess.player.use_gesture = crate::player::UseGesture::Spent;
        sess.player.inventory.decrement_held(eat.hand);
        for &(effect, ticks) in food.effects {
            sess.player.apply_effect(effect, ticks);
        }
        self.mods.emit(PostEvent::ItemUsed {
            player: self.sessions[s].id,
            item: eat.item,
            kind: ItemUseEvent::Eaten,
        });
    }

    pub fn try_use_item(
        &mut self,
        s: usize,
        click_target: Option<crate::net::protocol::TargetRef>,
        events: &mut TickEvents,
    ) -> bool {
        let Some(item) = self.sessions[s].player.held().map(|st| st.item) else {
            return false;
        };
        let target = click_target.map(|h| h.block);
        let mut pre = ItemUsePre { item, target };
        let cancelled = {
            let Self {
                world,
                sessions,
                mods,
                ..
            } = self;
            let actor = Some(sessions[s].id);
            let bus = mods.bus_mut();
            bus.item_use_pre(world, sessions, actor, events, &mut pre) == Outcome::Cancel
        };
        if cancelled {
            self.mods.emit(PostEvent::ItemUsed {
                player: self.sessions[s].id,
                item,
                kind: ItemUseEvent::Claimed,
            });
            return true;
        }
        let used = match rules::resolve_engine_item_use(&self.sessions[s].player, self.world.data())
        {
            Some(EngineItemUse::Fill { source, becomes }) => self.fill_bucket(s, source, becomes),
            Some(EngineItemUse::Pour {
                cell,
                fluid,
                becomes,
            }) => self.pour_bucket(s, cell, fluid, becomes, events),
            None => false,
        };
        if used {
            self.mods.emit(PostEvent::ItemUsed {
                player: self.sessions[s].id,
                item,
                kind: ItemUseEvent::Handler,
            });
        }
        used
    }

    pub fn try_shear_mob(&mut self, s: usize, target: Option<u64>) -> bool {
        if !rules::holds_shears(&self.sessions[s].player) {
            return false;
        }
        let Some(mob_id) =
            super::mob_target::authoritative_mob_target(&self.world, &self.sessions[s], target)
        else {
            return false;
        };
        let Some(ShearDrop {
            item,
            count,
            pos,
            skylight,
            blocklight,
        }) = self.world.mobs_mut().shear_mob(mob_id)
        else {
            return false;
        };
        let centre = pos + Vec3::new(0.0, 0.3, 0.0);
        let mut drop = DroppedItem::new(centre, ItemStack::new(item, count), self.seeds.draw());
        drop.skylight = skylight;
        drop.blocklight = blocklight;
        self.world.spawn_item(drop);
        true
    }

    fn fill_bucket(&mut self, s: usize, source: IVec3, becomes: ItemType) -> bool {
        let hand = self.sessions[s].player.acting_hand;
        if !self.sessions[s]
            .player
            .inventory
            .replace_held_one(hand, ItemStack::new(becomes, 1))
        {
            return false;
        }
        self.world
            .set_block_world(source.x, source.y, source.z, Block::Air);
        true
    }

    /// Pour the bucket into `p`. We don't pick the cell here; [`rules::bucket_pour_cell`] does. If
    /// it works you're left holding `becomes`, the empty bucket.
    fn pour_bucket(
        &mut self,
        s: usize,
        p: IVec3,
        fluid: Block,
        becomes: ItemType,
        events: &mut TickEvents,
    ) -> bool {
        let dir = self.sessions[s].player.forward();
        {
            let mut pre = BlockPlacePre {
                pos: p,
                block: fluid,
                facing: facing_from_forward(dir),
                actor: crate::mob::EntityRef::Player(self.sessions[s].id),
            };
            let Self {
                world,
                sessions,
                mods,
                ..
            } = self;
            let actor = Some(sessions[s].id);
            let bus = mods.bus_mut();
            let cancelled =
                bus.block_place_pre(world, sessions, actor, events, &mut pre) == Outcome::Cancel;
            if cancelled {
                return false;
            }
        }
        if !rules::pour_lands(self.world.data(), p) {
            return false;
        }
        if !self.world.set_block_world(p.x, p.y, p.z, fluid) {
            return false;
        }
        self.mods.emit(PostEvent::BlockPlaced {
            pos: p,
            block: fluid,
            player: Some(self.sessions[s].id),
        });
        self.push_block_noise(s, p, crate::mob::NoiseKind::BlockPlaced);
        let hand = self.sessions[s].player.acting_hand;
        self.sessions[s]
            .player
            .inventory
            .replace_held_one(hand, ItemStack::new(becomes, 1));
        true
    }
}
