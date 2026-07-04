//! Item-driven right-click actions — using the HELD ITEM on the world (the
//! buckets) or on the targeted mob (the shears), as opposed to placing a block or
//! using a clicked block's own capability. Runs on the fixed tick, dispatched from
//! `tick_place` after block interaction and before placement.

use super::placement::facing_from_forward;
use super::{tick::TickEvents, Game};
use crate::block::Block;
use crate::entity::DroppedItem;
use crate::events::{BlockPlacePre, ItemUsePre, Outcome, PostEvent};
use crate::item::{ItemStack, ItemType, ItemUse};
use crate::mathh::Vec3;
use crate::mob::ShearDrop;
use crate::player::Player;

impl Game {
    /// Apply the held item's own right-click use, if it has one. Returns `true`
    /// when the click was consumed: the world and the held item changed together.
    pub(super) fn try_use_item(&mut self, events: &mut TickEvents) -> bool {
        let Some(item) = self.player.inventory.selected().map(|s| s.item) else {
            return false;
        };
        // A handler cancelling `item_use_pre` consumed the click: the engine's own
        // use is skipped, but the item still reports as used (hand jab + post event).
        let mut pre = ItemUsePre {
            item,
            target: self.look.map(|h| h.block),
        };
        if self
            .bus
            .item_use_pre(&mut self.world, &mut self.player, events, &mut pre)
            == Outcome::Cancel
        {
            self.bus.emit(PostEvent::ItemUsed { item });
            return true;
        }
        // Dispatch on the item's data-declared use (`"use"` in items.json).
        // `Shear` acts at the earlier shear stage of `tick_place`; a `Pending`
        // (`mod_id:`) handler waits for the WASM host (Phase 2b).
        let used = match item.item_use() {
            Some(ItemUse::BucketFill) => self.try_fill_bucket(),
            Some(ItemUse::BucketPour) => self.try_pour_bucket(events),
            _ => false,
        };
        if used {
            self.bus.emit(PostEvent::ItemUsed { item });
        }
        used
    }

    /// Shear the targeted mob with the held shears: the mob's coat comes off (and
    /// starts regrowing) and its rolled drop pops at its body, like death loot.
    /// Returns `true` when the click was consumed. `targeted_mob` is already
    /// reach-limited by the per-frame target refresh, exactly like an attack.
    pub(super) fn try_shear_mob(&mut self) -> bool {
        if self.selected_item().and_then(ItemType::item_use) != Some(ItemUse::Shear) {
            return false;
        }
        let Some(idx) = self.targeted_mob else {
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

    /// Scoop water into the held empty bucket. The rule: the ray hits a water
    /// SOURCE within reach → that cell is scooped; otherwise nothing. The fill
    /// ray stops only at sources and solids — flowing water is transparent to
    /// it (like it is to normal selection), so a spread sheet or thin film,
    /// which can render exactly like still water, never shadows the source the
    /// player is actually aiming at, and aiming at pure flow does nothing.
    fn try_fill_bucket(&mut self) -> bool {
        let Some((h, _)) =
            Player::raycast_water_sources(self.cam.pos, self.cam.forward(), &self.world)
        else {
            return false;
        };
        if !self.world.is_water_source_world(h.block) {
            return false;
        }
        // The held-item swap must succeed BEFORE the world changes: with a full
        // inventory (nowhere for the filled bucket out of a stack) the scoop is
        // refused and the source stays.
        if !self
            .player
            .inventory
            .replace_selected_one(ItemStack::new(ItemType::WaterBucket, 1))
        {
            return false;
        }
        self.world
            .set_block_world(h.block.x, h.block.y, h.block.z, Block::Air);
        true
    }

    /// Empty the held water bucket into the clicked cell. The pour uses the same
    /// water-stopping ray as the fill, so aiming anywhere at a water body pours
    /// INTO its surface cell: flowing water firms into a source, and pouring
    /// onto an existing source still empties the bucket (a no-op world write) —
    /// on water the action is always predictable. On land it follows block
    /// placement: a replaceable target (grass, a fern) is filled in place,
    /// anything else pours against the clicked face.
    fn try_pour_bucket(&mut self, events: &mut TickEvents) -> bool {
        let Some((h, _)) =
            Player::raycast_including_water(self.cam.pos, self.cam.forward(), &self.world)
        else {
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
                facing: facing_from_forward(self.cam.forward()),
            };
            if self
                .bus
                .block_place_pre(&mut self.world, &mut self.player, events, &mut pre)
                == Outcome::Cancel
            {
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
        // A water bucket never stacks, so the swap back to the empty bucket is
        // always an in-place slot swap and cannot fail.
        self.player
            .inventory
            .replace_selected_one(ItemStack::new(ItemType::WoodenBucket, 1));
        true
    }
}
