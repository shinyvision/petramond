//! Item-driven right-click actions — using the HELD ITEM on the world (the
//! buckets), as opposed to placing a block or using a clicked block's own
//! capability. Runs on the fixed tick, dispatched from `tick_place` after block
//! interaction and before placement.

use super::Game;
use crate::block::Block;
use crate::item::{ItemStack, ItemType};
use crate::player::Player;

impl Game {
    /// Apply the held item's own right-click use, if it has one. Returns `true`
    /// when the click was consumed: the world and the held item changed together.
    pub(super) fn try_use_item(&mut self) -> bool {
        match self.player.inventory.selected().map(|s| s.item) {
            Some(ItemType::WoodenBucket) => self.try_fill_bucket(),
            Some(ItemType::WaterBucket) => self.try_pour_bucket(),
            _ => false,
        }
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
    fn try_pour_bucket(&mut self) -> bool {
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
        let target = Block::from_id(self.world.chunk_block(p.x, p.y, p.z));
        if !target.is_replaceable() {
            return false;
        }
        if !self.world.set_block_world(p.x, p.y, p.z, Block::Water) {
            return false;
        }
        // A water bucket never stacks, so the swap back to the empty bucket is
        // always an in-place slot swap and cannot fail.
        self.player
            .inventory
            .replace_selected_one(ItemStack::new(ItemType::WoodenBucket, 1));
        true
    }
}
