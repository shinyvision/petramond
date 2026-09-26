//! Item-driven right-click actions — using the HELD ITEM on the world (the
//! buckets) or on the targeted mob (the shears), as opposed to placing a block or
//! using a clicked block's own capability. Runs on the fixed tick, dispatched from
//! `tick_place` after block interaction and before placement.

use petramond_world::world::raycast;
use super::game::ServerGame;
use crate::entity::DroppedItem;
use crate::events::tick::TickEvents;
use crate::events::{BlockPlacePre, ItemUseEvent, ItemUsePre, Outcome, PostEvent};
use crate::mob::ShearDrop;
use crate::net::protocol::TargetRef;
use crate::player::Player;
use crate::rules::item_use::{self as rules, EngineItemUse};
use crate::rules::placement::facing_from_forward;
use petramond_math::math::{IVec3, Vec3};
use petramond_world::block::Block;
use petramond_world::item::{ItemStack, ItemType};

/// The in-progress eat: which hand and food item are being eaten and for how
/// many ticks the button has been held on it. Session-owned (one per player);
/// aborted the moment the button lifts or the eating hand's item changes. For
/// a MAIN-hand eat the SLOT is tracked so switching to a different slot
/// holding the same food still aborts (switching slots aborts); an OFF-hand
/// eat ignores hotbar selection entirely.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct EatingState {
    pub hand: petramond_world::inventory::Hand,
    /// The active hotbar slot at eat start. Meaningful for `Hand::Main` only.
    pub slot: u8,
    pub item: ItemType,
    pub progress: u32,
}

impl ServerGame {
    /// Validate a use click's claimed block at message receipt, preserving its
    /// click-time latch. `held_item` is the item captured into the same
    /// `PendingUseClick`, so the ray decision and the later selection guard
    /// share one identity. Ordinary solid-ray items retain the bounded target
    /// convention used by placement prediction. An item that explicitly asks
    /// for a fluid-stopping ray must additionally match the first hit of that
    /// same ray in the authoritative world; otherwise a client could name a
    /// different in-reach fluid cell behind an occluder.
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

        // The CELL and FACE are what the claim is judged on, never the spot:
        // the server re-casts from its own latched look, so its hit point
        // differs by a hair from the client's on every legitimate click.
        let authoritative = raycast::use_ray(eye, sess.player.forward(), self.world.data(), ray)
            .map(|(hit, _)| (hit.block, hit.normal));
        if claimed.map(|c| (c.block, c.normal)) == authoritative {
            claimed
        } else {
            None
        }
    }

    /// Start eating the held food item on a consumed secondary click. Returns
    /// `true` when the click belonged to food (whether the eat started, was
    /// cancelled by a mod, or was already running) so placement never fires
    /// from a food click. Fires `item_use_pre` at the START — a cancel eats
    /// the click, not the food.
    pub fn try_start_eating(&mut self, s: usize, events: &mut TickEvents) -> bool {
        let sess = &self.sessions[s];
        let hand = sess.player.acting_hand;
        // The shared eat gate (food in the acting hand, a body in play) — the
        // rule the client's jab prediction runs against its replica.
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
            return true; // re-click mid-eat: consumed, nothing restarts
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
            // The eating session acts.
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

    /// Advance the in-progress eat one tick (runs every tick, click or not):
    /// abort when the button lifted, the body was barred from using at all,
    /// the eating hand's item changed under the eat, or — for a main-hand eat
    /// — the selection moved to ANY other slot; consume the item and grant its
    /// effects when the hold reaches the row's `eat_ticks`.
    pub fn advance_eating(&mut self, s: usize) {
        let sess = &mut self.sessions[s];
        let Some(eat) = sess.sim.eating else {
            return;
        };
        let held = sess.player.inventory.held_in(eat.hand).map(|st| st.item);
        let selection_moved = eat.hand == petramond_world::inventory::Hand::Main
            && sess.player.inventory.active_slot() != eat.slot;
        // An eat is the use still happening, so barring the use ends it — a
        // meal that finished while the hands were tied would be the one thing
        // a denial let through.
        let barred = sess
            .player
            .denied_actions()
            .denies(mod_api::BodyAction::Use);
        if !sess.input.intent_use_held || barred || selection_moved || held != Some(eat.item) {
            sess.sim.eating = None;
            // The eat gave the gesture up; the button, if still down, is free
            // for the next interaction. (A release frees it in `tick_place`
            // anyway — this is the swap-mid-eat case.)
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
        // Done: the food leaves the hand and its effects land, atomically on
        // this tick. The gesture is SPENT, not freed — finishing is not the
        // same as letting go, and without the distinction a held button eats
        // the whole stack.
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

    /// Apply the held item's own right-click use, if it has one. Returns `true`
    /// when the click was consumed: the world and the held item changed together.
    pub fn try_use_item(
        &mut self,
        s: usize,
        click_target: Option<crate::net::protocol::TargetRef>,
        events: &mut TickEvents,
    ) -> bool {
        let Some(item) = self.sessions[s].player.held().map(|st| st.item) else {
            return false;
        };
        // A handler cancelling `item_use_pre` consumed the click: the engine's own
        // use is skipped, but the item still reports as used (hand jab + post event).
        let target = click_target.map(|h| h.block);
        let mut pre = ItemUsePre { item, target };
        let cancelled = {
            let Self {
                world,
                sessions,
                mods,
                ..
            } = self;
            // The clicking session acts.
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
        // The item's data-declared use (`"use"` in items.json), resolved by
        // the SHARED rule the client predicts with — handler params (the
        // bucket counterpart) ride the row, so a pack bucket transitions
        // within its own item pair. `Shear` acts at the earlier shear stage;
        // mod items react to use through the `item_use_pre` event above.
        let used = match rules::resolve_engine_item_use(&self.sessions[s].player, self.world.data()) {
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

    /// Shear the targeted mob with the held shears: the mob's coat comes off (and
    /// starts regrowing) and its rolled drop pops at its body, like death loot.
    /// Returns `true` when the click was consumed. `target` is the stable mob id
    /// the `UseClick` claimed; the authoritative view-ray validator resolves
    /// it before mutation. A forged or vanished target is a no-op.
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
        // Pop from roughly the mob's body centre, like death loot.
        let centre = pos + Vec3::new(0.0, 0.3, 0.0);
        let mut drop = DroppedItem::new(centre, ItemStack::new(item, count), self.seeds.draw());
        drop.skylight = skylight;
        drop.blocklight = blocklight;
        self.world.spawn_item(drop);
        true
    }

    /// Scoop the fluid source the shared fill rule resolved
    /// ([`rules::bucket_fill_target`]: a still source of a fluid the bucket
    /// takes, reached through flow) into the held empty bucket; on success
    /// the held item becomes `becomes`, the result the row declares for the
    /// scooped fluid.
    fn fill_bucket(&mut self, s: usize, source: IVec3, becomes: ItemType) -> bool {
        // The held-item swap must succeed BEFORE the world changes: with a full
        // inventory (nowhere for the filled bucket out of a stack) the scoop is
        // refused and the source stays.
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

    /// Empty the held bucket as `fluid` into `p`, the cell the shared pour rule
    /// resolved ([`rules::bucket_pour_cell`]: the first cell of ANY fluid is
    /// poured into — flowing water firms into a source, the other fluid swaps
    /// the surface cell for the fluid sim's contact rule — and on land a
    /// replaceable target fills in place, anything else pours against the
    /// clicked face). On success the held item becomes `becomes`, the
    /// row-declared empty counterpart.
    fn pour_bucket(
        &mut self,
        s: usize,
        p: IVec3,
        fluid: Block,
        becomes: ItemType,
        events: &mut TickEvents,
    ) -> bool {
        let dir = self.sessions[s].player.forward();
        // Pouring places a water block, so it announces the same `block_place_pre`
        // a held block would; cancel = the pour is refused, the bucket kept full.
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
            // The pouring session acts.
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
        // A filled bucket row is max-stack 1 (the engine's water bucket; packs
        // should declare theirs the same), so the swap back to the empty
        // counterpart is an in-place slot swap and cannot fail.
        let hand = self.sessions[s].player.acting_hand;
        self.sessions[s]
            .player
            .inventory
            .replace_held_one(hand, ItemStack::new(becomes, 1));
        true
    }
}
