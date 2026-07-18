//! Item-driven right-click actions — using the HELD ITEM on the world (the
//! buckets) or on the targeted mob (the shears), as opposed to placing a block or
//! using a clicked block's own capability. Runs on the fixed tick, dispatched from
//! `tick_place` after block interaction and before placement.

use super::game::ServerGame;
use super::placement::facing_from_forward;
use crate::block::Block;
use crate::entity::DroppedItem;
use crate::events::{BlockPlacePre, ItemUsePre, MobInteract, Outcome, PostEvent};
use crate::game::tick::TickEvents;
use crate::item::{ItemStack, ItemType, ItemUse, UseRay};
use crate::mathh::Vec3;
use crate::mob::ShearDrop;
use crate::net::protocol::TargetRef;
use crate::player::Player;

/// The in-progress eat: which hotbar slot and food item are being eaten and
/// for how many ticks the button has been held on it. Session-owned (one per
/// player); aborted the moment the button lifts or the hotbar selection
/// changes — the SLOT is tracked so switching to a different slot holding the
/// same food still aborts (switching slots aborts).
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct EatingState {
    pub(crate) slot: u8,
    pub(crate) item: ItemType,
    pub(crate) progress: u32,
}

impl ServerGame {
    /// Validate a use click's claimed block at message receipt, preserving its
    /// click-time latch. `held_item` is the item captured into the same
    /// `PendingUseClick`, so the ray decision and the later selection guard
    /// share one identity. Ordinary solid-ray items retain the bounded target
    /// convention used by placement prediction. An item that explicitly asks
    /// for a water-stopping ray must additionally match the first hit of that
    /// same ray in the authoritative world; otherwise a client could name a
    /// different in-reach water cell behind an occluder.
    pub(crate) fn authoritative_use_target(
        &self,
        s: usize,
        held_item: Option<ItemType>,
        claimed: Option<TargetRef>,
    ) -> Option<TargetRef> {
        let sess = &self.sessions[s];
        let eye = super::movement::reach_eye(sess);
        let claimed = claimed.filter(|target| crate::player::block_within_reach(eye, target.block));
        if held_item.is_none_or(|item| item.use_ray() != UseRay::Water) {
            return claimed;
        }

        let authoritative =
            Player::raycast_including_water(eye, sess.player.forward(), &self.world).map(
                |(hit, _)| TargetRef {
                    block: hit.block,
                    normal: hit.normal,
                },
            );
        if claimed == authoritative {
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
    pub(crate) fn try_start_eating(&mut self, s: usize, events: &mut TickEvents) -> bool {
        let sess = &self.sessions[s];
        let Some(item) = sess.player.inventory.selected().map(|st| st.item) else {
            return false;
        };
        if item.food().is_none() || sess.player.is_spectator() {
            return false;
        }
        let slot = sess.player.inventory.active_slot();
        if sess
            .eating
            .is_some_and(|e| e.slot == slot && e.item == item)
        {
            return true; // re-click mid-eat: consumed, nothing restarts
        }
        let target = sess.look.map(|h| h.block);
        let mut pre = ItemUsePre { item, target };
        let cancelled = {
            let Self {
                world,
                sessions,
                bus,
                ..
            } = self;
            // The eating session acts; the sessions view rides the dispatch.
            Self::with_sessions_view(sessions, s, |sess| {
                bus.item_use_pre(
                    world,
                    &mut sess.player,
                    &mut sess.gui_state,
                    events,
                    &mut pre,
                ) == Outcome::Cancel
            })
        };
        if cancelled {
            self.bus.emit(PostEvent::ItemUsed { item });
            return true;
        }
        self.sessions[s].eating = Some(EatingState {
            slot,
            item,
            progress: 0,
        });
        true
    }

    /// Advance the in-progress eat one tick (runs every tick, click or not):
    /// abort when the button lifted, the selection moved to ANY other slot,
    /// or the slot's item changed under the eat; consume the item and grant
    /// its effects when the hold reaches the row's `eat_ticks`.
    pub(crate) fn advance_eating(&mut self, s: usize, events: &mut TickEvents) {
        let sess = &mut self.sessions[s];
        let Some(eat) = sess.eating else {
            return;
        };
        let held = sess.player.inventory.selected().map(|st| st.item);
        if !sess.intent_use_held
            || sess.player.inventory.active_slot() != eat.slot
            || held != Some(eat.item)
        {
            sess.eating = None;
            return;
        }
        let Some(food) = eat.item.food() else {
            sess.eating = None;
            return;
        };
        let progress = eat.progress + 1;
        if progress < food.eat_ticks {
            sess.eating = Some(EatingState { progress, ..eat });
            return;
        }
        // Done: the food leaves the hotbar and its effects land, atomically on
        // this tick.
        sess.eating = None;
        sess.player.inventory.decrement_selected();
        for &(effect, ticks) in food.effects {
            sess.player.apply_effect(effect, ticks);
        }
        events.player(s).ate_finished = true;
        self.bus.emit(PostEvent::ItemUsed { item: eat.item });
    }

    /// Apply the held item's own right-click use, if it has one. Returns `true`
    /// when the click was consumed: the world and the held item changed together.
    pub(crate) fn try_use_item(
        &mut self,
        s: usize,
        click_target: Option<crate::net::protocol::TargetRef>,
        events: &mut TickEvents,
    ) -> bool {
        let Some(item) = self.sessions[s]
            .player
            .inventory
            .selected()
            .map(|st| st.item)
        else {
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
                bus,
                ..
            } = self;
            // The clicking session acts; the sessions view rides the dispatch.
            Self::with_sessions_view(sessions, s, |sess| {
                bus.item_use_pre(
                    world,
                    &mut sess.player,
                    &mut sess.gui_state,
                    events,
                    &mut pre,
                ) == Outcome::Cancel
            })
        };
        if cancelled {
            self.bus.emit(PostEvent::ItemUsed { item });
            return true;
        }
        // Dispatch on the item's data-declared use (`"use"` in items.json) —
        // handler params (the bucket counterpart) ride the row, so a pack
        // bucket transitions within its own item pair. `Shear` acts at the
        // earlier shear stage of `tick_place`; mod items react to use through
        // the `item_use_pre` event handled above.
        let used = match item.item_use() {
            Some(ItemUse::BucketFill { becomes }) => self.try_fill_bucket(s, becomes),
            Some(ItemUse::BucketPour { becomes }) => self.try_pour_bucket(s, becomes, events),
            _ => false,
        };
        if used {
            self.bus.emit(PostEvent::ItemUsed { item });
        }
        used
    }

    /// Dispatch `mob_interact` for a use click whose crosshair target was a
    /// live mob — mods see the interaction before any engine mob use
    /// (shears), mirroring how `block_interact` precedes engine block
    /// capabilities. Returns `true` when a handler consumed the click.
    /// `target` is only the stable mob id the `UseClick` claimed. It must
    /// resolve through the authoritative view-ray validator before the event
    /// can observe it; a forged, vanished, dead, or occluded target is a
    /// no-op.
    pub(crate) fn mob_interact(
        &mut self,
        s: usize,
        target: Option<u64>,
        events: &mut TickEvents,
    ) -> bool {
        let Some(idx) = self.authoritative_mob_target(s, target) else {
            return false;
        };
        let inst = &self.world.mobs().instances()[idx];
        let mut pre = MobInteract {
            id: inst.id(),
            kind: inst.kind,
            player: self.sessions[s].id,
        };
        let Self {
            world,
            sessions,
            bus,
            ..
        } = self;
        // The interacting session acts; the sessions view rides the dispatch.
        Self::with_sessions_view(sessions, s, |sess| {
            bus.mob_interact(
                world,
                &mut sess.player,
                &mut sess.gui_state,
                events,
                &mut pre,
            ) == Outcome::Cancel
        })
    }

    /// Shear the targeted mob with the held shears: the mob's coat comes off (and
    /// starts regrowing) and its rolled drop pops at its body, like death loot.
    /// Returns `true` when the click was consumed. `target` is the stable mob id
    /// the `UseClick` claimed; the authoritative view-ray validator resolves
    /// it before mutation. A forged or vanished target is a no-op.
    pub(crate) fn try_shear_mob(&mut self, s: usize, target: Option<u64>) -> bool {
        if self.sessions[s]
            .selected_item()
            .and_then(ItemType::item_use)
            != Some(ItemUse::Shear)
        {
            return false;
        }
        let Some(idx) = self.authoritative_mob_target(s, target) else {
            return false;
        };
        let Some(ShearDrop {
            item,
            count,
            pos,
            skylight,
            blocklight,
        }) = self.world.mobs_mut().shear_mob(idx)
        else {
            return false;
        };
        // Pop from roughly the mob's body centre, like death loot.
        let centre = pos + Vec3::new(0.0, 0.3, 0.0);
        self.spawn_counter = self.spawn_counter.wrapping_add(1);
        let mut drop = DroppedItem::new(centre, ItemStack::new(item, count), self.spawn_counter);
        drop.skylight = skylight;
        drop.blocklight = blocklight;
        self.world.spawn_item(drop);
        true
    }

    /// Scoop water into the held empty bucket; on success the held item
    /// becomes `becomes`, the row-declared filled counterpart. The rule: the
    /// ray hits a water SOURCE within reach → that cell is scooped; otherwise
    /// nothing. The fill ray stops only at sources and solids — flowing water
    /// is transparent to it (like it is to normal selection), so a spread
    /// sheet or thin film, which can render exactly like still water, never
    /// shadows the source the player is actually aiming at, and aiming at
    /// pure flow does nothing.
    fn try_fill_bucket(&mut self, s: usize, becomes: ItemType) -> bool {
        let (eye, dir) = {
            let p = &self.sessions[s].player;
            (p.eye(), p.forward())
        };
        let Some((h, _)) = Player::raycast_water_sources(eye, dir, &self.world) else {
            return false;
        };
        if !self.world.is_water_source_world(h.block) {
            return false;
        }
        // The held-item swap must succeed BEFORE the world changes: with a full
        // inventory (nowhere for the filled bucket out of a stack) the scoop is
        // refused and the source stays.
        if !self.sessions[s]
            .player
            .inventory
            .replace_selected_one(ItemStack::new(becomes, 1))
        {
            return false;
        }
        self.world
            .set_block_world(h.block.x, h.block.y, h.block.z, Block::Air);
        true
    }

    /// Empty the held water bucket into the clicked cell; on success the held
    /// item becomes `becomes`, the row-declared empty counterpart. The pour
    /// uses the same water-stopping ray as the fill, so aiming anywhere at a
    /// water body pours INTO its surface cell: flowing water firms into a
    /// source, and pouring onto an existing source still empties the bucket (a
    /// no-op world write) — on water the action is always predictable. On land
    /// it follows block placement: a replaceable target (grass, a fern) is
    /// filled in place, anything else pours against the clicked face.
    fn try_pour_bucket(&mut self, s: usize, becomes: ItemType, events: &mut TickEvents) -> bool {
        let (eye, dir) = {
            let p = &self.sessions[s].player;
            (p.eye(), p.forward())
        };
        let Some((h, _)) = Player::raycast_including_water(eye, dir, &self.world) else {
            return false;
        };
        // Water is itself replaceable, so a water hit pours in place.
        let looked_at = Block::from_id(self.world.chunk_block(h.block.x, h.block.y, h.block.z));
        let p = if looked_at.is_replaceable() && looked_at != Block::Air {
            h.block
        } else {
            if h.normal == crate::mathh::IVec3::ZERO {
                return false;
            }
            h.block + h.normal
        };
        // Pouring places a water block, so it announces the same `block_place_pre`
        // a held block would; cancel = the pour is refused, the bucket kept full.
        {
            let mut pre = BlockPlacePre {
                pos: p,
                block: Block::Water,
                facing: facing_from_forward(dir),
            };
            let Self {
                world,
                sessions,
                bus,
                ..
            } = self;
            // The pouring session acts; the sessions view rides the dispatch.
            let cancelled = Self::with_sessions_view(sessions, s, |sess| {
                bus.block_place_pre(
                    world,
                    &mut sess.player,
                    &mut sess.gui_state,
                    events,
                    &mut pre,
                ) == Outcome::Cancel
            });
            if cancelled {
                return false;
            }
        }
        let target = Block::from_id(self.world.chunk_block(p.x, p.y, p.z));
        if !target.is_replaceable() {
            return false;
        }
        if !self.world.set_block_world(p.x, p.y, p.z, Block::Water) {
            return false;
        }
        self.bus.emit(PostEvent::BlockPlaced {
            pos: p,
            block: Block::Water,
        });
        self.push_block_noise(s, p, crate::mob::NoiseKind::BlockPlaced);
        // A filled bucket row is max-stack 1 (the engine's water bucket; packs
        // should declare theirs the same), so the swap back to the empty
        // counterpart is an in-place slot swap and cannot fail.
        self.sessions[s]
            .player
            .inventory
            .replace_selected_one(ItemStack::new(becomes, 1));
        true
    }
}
