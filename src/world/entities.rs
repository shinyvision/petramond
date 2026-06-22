//! Dropped item-stack entities, owned by the world alongside the chunks they
//! rest in.
//!
//! Each drop carries a tick lifetime (`DroppedItem::ticks_lived`). The timer is
//! advanced once per fixed game tick by [`World::tick_item_lifetime`], and an
//! item is removed when it reaches [`ITEM_LIFETIME_TICKS`]. Because an item lives
//! only while its chunk is loaded — it unloads into the chunk's save record and
//! reloads from it (see `world::stream` / `world::store`) — the timer naturally
//! *pauses* while the chunk is gone and resumes with the right remaining time.
//!
//! Performance: the active list holds only drops in currently-loaded chunks, so
//! it never grows with far-flung frozen items; per-tick work is bounded by what
//! the player can actually see. Physics ticks against an immutable `&World` via a
//! `mem::take` of the list, keeping the borrow split clean.

use std::collections::HashMap;

use crate::chunk::ChunkPos;
use crate::entity::DroppedItem;
use crate::item::ItemStack;
use crate::mathh::{IVec3, Vec3};

use super::store::World;

/// Item entity lifetime: 6000 game ticks (5 minutes at 20 TPS). The timer only
/// advances while the holding chunk is loaded, and persists with the chunk, so an
/// item that has lived 3000 ticks still has 3000 ticks left after a reload.
pub const ITEM_LIFETIME_TICKS: u32 = 6000;

/// Ticks a freshly dropped/thrown item must live before it can be vacuumed up: 10
/// ticks (0.5 s at 20 TPS), so a just-thrown stack flies clear before the magnet
/// can pull it back.
pub const ITEM_PICKUP_DELAY_TICKS: u32 = 10;

impl World {
    /// Add a dropped item to the active set (it must lie in a loaded chunk).
    pub fn spawn_item(&mut self, item: DroppedItem) {
        self.dropped.push(item);
    }

    /// The active dropped items, for the renderer's per-frame instance mapping.
    pub fn item_entities(&self) -> &[DroppedItem] {
        &self.dropped
    }

    /// Mutable access to the active item list, for tests that seed or trim it.
    #[cfg(test)]
    pub fn item_entities_mut(&mut self) -> &mut Vec<DroppedItem> {
        &mut self.dropped
    }

    /// Per-frame physics for active items: gravity, collision, spin, and the
    /// pickup magnet toward `magnet_target` (the player chest). Drops past the
    /// pickup delay are magnetised. Skylight is refreshed only when a drop crosses
    /// a voxel cell. With a save attached, a drop sitting over a not-yet-loaded
    /// chunk is frozen so it can't fall through missing terrain (in-memory worlds
    /// with no save always simulate, matching the test setups).
    pub fn tick_item_physics(&mut self, dt: f32, magnet_target: Vec3) {
        if self.dropped.is_empty() {
            return;
        }
        let freeze_unloaded = self.save.is_some();
        // Detach the list so physics can read the rest of the world immutably.
        let mut items = std::mem::take(&mut self.dropped);
        for it in &mut items {
            if freeze_unloaded {
                let (cx, cz) = chunk_xz(it.pos);
                if !self.chunk_loaded(cx, cz) {
                    continue;
                }
            }
            let magnet = (it.ticks_lived >= ITEM_PICKUP_DELAY_TICKS).then_some(magnet_target);
            let before = voxel_at(it.pos);
            it.tick(dt, self, magnet);
            let after = voxel_at(it.pos);
            if before != after {
                it.skylight = self.skylight6_at_world(after.x, after.y, after.z);
            }
        }
        self.dropped = items;
    }

    /// Per fixed game-tick lifetime step: age each active item by one tick and
    /// despawn those that reach [`ITEM_LIFETIME_TICKS`]. With a save attached, an
    /// item over an unloaded chunk is paused (its timer does not advance) as a
    /// safety net for a drop that drifted to the streamed edge before unload could
    /// harvest it.
    pub fn tick_item_lifetime(&mut self) {
        let pause_unloaded = self.save.is_some();
        let mut i = self.dropped.len();
        while i > 0 {
            i -= 1;
            if pause_unloaded {
                let (cx, cz) = chunk_xz(self.dropped[i].pos);
                if !self.chunk_loaded(cx, cz) {
                    continue;
                }
            }
            let lived = self.dropped[i].ticks_lived.saturating_add(1);
            self.dropped[i].ticks_lived = lived;
            if lived >= ITEM_LIFETIME_TICKS {
                self.dropped.swap_remove(i);
            }
        }
    }

    /// Per fixed game-tick pickup: offer each eligible item (past the pickup delay
    /// and within the player's absorb radius) to `deposit`, which returns any
    /// leftover that didn't fit the inventory. Fully absorbed items are removed.
    /// `deposit` is a closure so the world stays decoupled from the inventory.
    pub fn collect_item_pickups(
        &mut self,
        player_pos: Vec3,
        mut deposit: impl FnMut(ItemStack) -> Option<ItemStack>,
    ) {
        let mut i = self.dropped.len();
        while i > 0 {
            i -= 1;
            if self.dropped[i].ticks_lived < ITEM_PICKUP_DELAY_TICKS {
                continue;
            }
            if self.dropped[i].within_pickup(player_pos) {
                match deposit(self.dropped[i].stack) {
                    None => {
                        self.dropped.swap_remove(i);
                    }
                    Some(leftover) => {
                        self.dropped[i].stack = leftover;
                    }
                }
            }
        }
    }

    /// Recompute every active item's cached skylight (after a world light update).
    pub fn refresh_item_lights(&mut self) {
        if self.dropped.is_empty() {
            return;
        }
        let mut items = std::mem::take(&mut self.dropped);
        for it in &mut items {
            let c = voxel_at(it.pos);
            it.skylight = self.skylight6_at_world(c.x, c.y, c.z);
        }
        self.dropped = items;
    }

    /// Drain and return the active items resting in chunk `pos` — used to bundle
    /// them into that chunk's save record as it unloads.
    pub(super) fn take_items_in_chunk(&mut self, pos: ChunkPos) -> Vec<DroppedItem> {
        let mut taken = Vec::new();
        let mut i = self.dropped.len();
        while i > 0 {
            i -= 1;
            let (cx, cz) = chunk_xz(self.dropped[i].pos);
            if cx == pos.cx && cz == pos.cz {
                taken.push(self.dropped.swap_remove(i));
            }
        }
        taken
    }

    /// Clone the active items grouped by owning chunk, for the periodic save
    /// flush (the items stay active; the clones persist with the chunk records so
    /// a crash can't lose their lifetimes).
    pub(super) fn items_by_chunk(&self) -> HashMap<ChunkPos, Vec<DroppedItem>> {
        let mut map: HashMap<ChunkPos, Vec<DroppedItem>> = HashMap::new();
        for it in &self.dropped {
            let (cx, cz) = chunk_xz(it.pos);
            map.entry(ChunkPos::new(cx, cz)).or_default().push(it.clone());
        }
        map
    }
}

#[inline]
fn voxel_at(pos: Vec3) -> IVec3 {
    IVec3::new(
        pos.x.floor() as i32,
        pos.y.floor() as i32,
        pos.z.floor() as i32,
    )
}

/// Chunk coordinates owning world position `pos`.
#[inline]
fn chunk_xz(pos: Vec3) -> (i32, i32) {
    ((pos.x.floor() as i32) >> 4, (pos.z.floor() as i32) >> 4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::ItemType;

    fn drop_at(x: f32, z: f32) -> DroppedItem {
        DroppedItem::new(Vec3::new(x, 64.0, z), ItemStack::new(ItemType::Dirt, 1), 1)
    }

    #[test]
    fn lifetime_advances_and_despawns_at_the_limit() {
        // No save attached, so the timer never pauses — it just counts up.
        let mut w = World::new(0, 0);
        let mut item = drop_at(0.5, 0.5);
        item.ticks_lived = ITEM_LIFETIME_TICKS - 2;
        w.spawn_item(item);
        w.tick_item_lifetime();
        assert_eq!(w.item_entities()[0].ticks_lived, ITEM_LIFETIME_TICKS - 1);
        w.tick_item_lifetime();
        assert!(
            w.item_entities().is_empty(),
            "despawns once it reaches the lifetime limit"
        );
    }

    #[test]
    fn pickup_waits_out_the_delay_then_collects() {
        let mut w = World::new(0, 0);
        let player = Vec3::new(0.5, 64.0, 0.5);
        w.spawn_item(drop_at(0.5, 0.5)); // ticks_lived 0: inside the delay window
        let mut collected = 0u32;
        w.collect_item_pickups(player, |s| {
            collected += s.count as u32;
            None
        });
        assert_eq!(collected, 0, "the pickup delay blocks collection");
        assert_eq!(w.item_entities().len(), 1);

        w.item_entities_mut()[0].ticks_lived = ITEM_PICKUP_DELAY_TICKS;
        w.collect_item_pickups(player, |s| {
            collected += s.count as u32;
            None
        });
        assert_eq!(collected, 1, "collected once past the delay");
        assert!(w.item_entities().is_empty());
    }

    #[test]
    fn pickup_keeps_leftover_that_did_not_fit() {
        let mut w = World::new(0, 0);
        let player = Vec3::new(0.5, 64.0, 0.5);
        let mut item =
            DroppedItem::new(player, ItemStack::new(ItemType::Dirt, 10), 1);
        item.ticks_lived = ITEM_PICKUP_DELAY_TICKS;
        w.spawn_item(item);
        // The "inventory" only accepts part of the stack, returning a leftover.
        w.collect_item_pickups(player, |s| Some(ItemStack::new(s.item, 4)));
        assert_eq!(w.item_entities().len(), 1, "leftover stays in the world");
        assert_eq!(w.item_entities()[0].stack.count, 4);
    }

    #[test]
    fn unloading_a_chunk_harvests_only_its_items() {
        // take_items_in_chunk is what an unload uses to bundle a chunk's drops
        // into its save record (and so pause their timers).
        let mut w = World::new(0, 0);
        w.spawn_item(drop_at(2.5, 2.5)); // chunk (0, 0)
        w.spawn_item(drop_at(20.5, 2.5)); // chunk (1, 0)
        let taken = w.take_items_in_chunk(ChunkPos::new(0, 0));
        assert_eq!(taken.len(), 1, "only the (0,0) drop is harvested");
        assert_eq!(w.item_entities().len(), 1, "the (1,0) drop stays active");
        assert!(w.item_entities()[0].pos.x > 16.0);
    }

    #[test]
    fn items_group_by_owning_chunk_for_flush() {
        let mut w = World::new(0, 0);
        w.spawn_item(drop_at(2.5, 2.5)); // (0, 0)
        w.spawn_item(drop_at(5.5, 9.5)); // (0, 0)
        w.spawn_item(drop_at(20.5, 2.5)); // (1, 0)
        let map = w.items_by_chunk();
        assert_eq!(map[&ChunkPos::new(0, 0)].len(), 2);
        assert_eq!(map[&ChunkPos::new(1, 0)].len(), 1);
    }
}
