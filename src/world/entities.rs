//! Dropped item-stack entities, owned by the world alongside the chunks they
//! rest in.
//!
//! Each drop carries a tick lifetime (`DroppedItem::ticks_lived`). The timer is
//! advanced once per fixed game tick by [`DroppedItems::tick_lifetime`], and an
//! item is removed when it reaches [`ITEM_LIFETIME_TICKS`]. Because an item lives
//! only while its chunk is loaded — it unloads into the chunk's save record and
//! reloads from it (see `world::stream` / `world::store`) — the timer naturally
//! *pauses* while the chunk is gone and resumes with the right remaining time.
//!
//! Performance: the active list holds only drops in currently-loaded chunks, so
//! it never grows with far-flung frozen items; per-tick work is bounded by what
//! the player can actually see. Physics ticks against an immutable `&World` via a
//! `mem::take` of the list, keeping the borrow split clean.
//!
//! [`DroppedItems`] owns the active `Vec<DroppedItem>` and all of the management
//! logic. It is **stateless with respect to `World`**: it stores no `&World`/
//! `&mut World` borrow. The methods that need world access (loaded-chunk checks
//! and skylight sampling) take the `&World` as a parameter per call; `World`
//! drives them by temporarily moving the field out so the two borrows stay
//! disjoint (see `World::tick_item_physics` and friends in this file).

use std::collections::HashMap;

use crate::chunk::SectionPos;
use crate::entity::DroppedItem;
use crate::item::ItemStack;
use crate::mathh::{voxel_at, Vec3};

use super::store::World;

/// Item entity lifetime: 6000 game ticks (5 minutes at 20 TPS). The timer only
/// advances while the holding chunk is loaded, and persists with the chunk, so an
/// item that has lived 3000 ticks still has 3000 ticks left after a reload.
pub const ITEM_LIFETIME_TICKS: u32 = 6000;

/// Ticks a freshly dropped/thrown item must live before it can be vacuumed up: 10
/// ticks (0.5 s at 20 TPS), so a just-thrown stack flies clear before the magnet
/// can pull it back.
pub const ITEM_PICKUP_DELAY_TICKS: u32 = 10;

/// The world's active dropped-item entities: those resting in currently-loaded
/// chunks. Owns the backing `Vec<DroppedItem>` and the entity-subsystem logic
/// (physics ticking, pickup planning/splitting, lifetime/despawn, and the
/// save-bundling helpers).
///
/// `DroppedItems` is **stateless with respect to `World`**: it holds no borrow of
/// a world. Methods that read the world (loaded-chunk checks, skylight) take the
/// `&World` they operate on as a parameter, so `World` can hand them `&self`
/// without ever storing the borrow — see [`World::tick_item_physics`].
#[derive(Default)]
pub struct DroppedItems {
    items: Vec<DroppedItem>,
}

impl DroppedItems {
    /// Add a dropped item to the active set (it must lie in a loaded chunk).
    pub fn spawn(&mut self, item: DroppedItem) {
        self.items.push(item);
    }

    /// The active dropped items, for the renderer's per-frame instance mapping.
    pub fn items(&self) -> &[DroppedItem] {
        &self.items
    }

    /// Mutable access to the active item list, for tests that seed or trim it.
    #[cfg(test)]
    pub fn items_mut(&mut self) -> &mut Vec<DroppedItem> {
        &mut self.items
    }

    /// Whether there are no active drops (lets `World` skip the take/restore dance
    /// without exposing the backing list).
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Per-frame physics for active items: gravity, collision, spin, and the
    /// pickup magnet toward `magnet_target` (the player chest). Only drops marked
    /// by [`request_pickups`](Self::request_pickups) are magnetised, so inventory
    /// capacity is planned before movement starts.
    ///
    /// Skylight is refreshed only when a drop crosses a voxel cell. When
    /// `freeze_unloaded` is set (a save is attached), a drop sitting over a
    /// not-yet-loaded chunk is frozen so it can't fall through missing terrain
    /// (in-memory worlds with no save always simulate, matching the test setups).
    ///
    /// Takes `world` (immutable) as a parameter; the caller must not be holding a
    /// borrow of these `DroppedItems` through the same `World`.
    pub fn tick_physics(
        &mut self,
        world: &World,
        dt: f32,
        magnet_target: Vec3,
        freeze_unloaded: bool,
    ) {
        for it in &mut self.items {
            if freeze_unloaded {
                let (cx, cz) = chunk_xz(it.pos);
                if !world.chunk_loaded(cx, cz) {
                    continue;
                }
            }
            let magnet = it.pickup_requested.then_some(magnet_target);
            let before = voxel_at(it.pos);
            it.tick(dt, world, magnet);
            let after = voxel_at(it.pos);
            if before != after {
                it.skylight = world.combined_light6_at_world(after.x, after.y, after.z);
            }
        }
    }

    /// Per fixed game-tick lifetime step: age each active item by one tick and
    /// despawn those that reach [`ITEM_LIFETIME_TICKS`]. When `pause_unloaded` is
    /// set (a save is attached), an item over an unloaded chunk is paused (its
    /// timer does not advance) as a safety net for a drop that drifted to the
    /// streamed edge before unload could harvest it.
    pub fn tick_lifetime(&mut self, world: &World, pause_unloaded: bool) {
        let mut i = self.items.len();
        while i > 0 {
            i -= 1;
            if pause_unloaded {
                let (cx, cz) = chunk_xz(self.items[i].pos);
                if !world.chunk_loaded(cx, cz) {
                    continue;
                }
            }
            let lived = self.items[i].ticks_lived.saturating_add(1);
            self.items[i].ticks_lived = lived;
            if lived >= ITEM_LIFETIME_TICKS {
                self.items.swap_remove(i);
            }
        }
    }

    /// Per fixed game-tick pickup planning. Eligible drops are those past the
    /// pickup delay and inside the player's attract radius. For each candidate,
    /// `request` returns how many items are reserved by the inventory simulation:
    /// `0` leaves the drop alone, the full count requests the whole entity, and a
    /// partial count splits that many items into a requested entity while leaving
    /// the remainder unrequested.
    ///
    /// Already-requested drops are planned first. That keeps a split-off stack from
    /// being duplicated every tick while it is flying toward the player.
    pub fn request_pickups(&mut self, player_pos: Vec3, mut request: impl FnMut(ItemStack) -> u8) {
        let was_requested: Vec<bool> = self.items.iter().map(|d| d.pickup_requested).collect();
        let mut split_offs = Vec::new();

        for (i, &requested) in was_requested.iter().enumerate() {
            if !requested {
                continue;
            }
            if !self.pickup_request_candidate(i, player_pos) {
                self.items[i].clear_pickup_request();
                continue;
            }
            let count = request(self.items[i].stack).min(self.items[i].stack.count);
            if count == 0 {
                self.items[i].clear_pickup_request();
            } else {
                self.apply_pickup_request(i, count, &mut split_offs);
            }
        }

        for (i, &requested) in was_requested.iter().enumerate() {
            if requested || !self.pickup_request_candidate(i, player_pos) {
                continue;
            }
            let count = request(self.items[i].stack).min(self.items[i].stack.count);
            if count > 0 {
                self.apply_pickup_request(i, count, &mut split_offs);
            }
        }

        self.items.extend(split_offs);
    }

    /// Per fixed game-tick pickup absorption. Only requested drops can be
    /// collected. `deposit` returns any leftover that did not fit; a leftover drop
    /// has its request cleared so the next planner pass can decide what to do.
    pub fn collect_requested_pickups(
        &mut self,
        player_pos: Vec3,
        mut deposit: impl FnMut(ItemStack) -> Option<ItemStack>,
    ) {
        let mut i = self.items.len();
        while i > 0 {
            i -= 1;
            if !self.items[i].pickup_requested {
                continue;
            }
            if !self.items[i].within_pickup(player_pos) {
                continue;
            }
            match deposit(self.items[i].stack) {
                None => {
                    self.items.swap_remove(i);
                }
                Some(leftover) if leftover.is_empty() => {
                    self.items.swap_remove(i);
                }
                Some(leftover) => {
                    self.items[i].stack = leftover;
                    self.items[i].clear_pickup_request();
                }
            }
        }
    }

    fn pickup_request_candidate(&self, i: usize, player_pos: Vec3) -> bool {
        let item = &self.items[i];
        item.ticks_lived >= ITEM_PICKUP_DELAY_TICKS
            && !item.stack.is_empty()
            && item.within_attract(player_pos)
    }

    fn apply_pickup_request(&mut self, i: usize, count: u8, split_offs: &mut Vec<DroppedItem>) {
        debug_assert!(count > 0);
        let stack_count = self.items[i].stack.count;
        if count >= stack_count {
            self.items[i].request_pickup();
            return;
        }

        // Clone the full physics state so the requested part starts exactly where
        // the source stack is. The remainder is left unrequested and therefore
        // will not be pulled by the magnet.
        let mut split = self.items[i].clone();
        split.stack.count = count;
        split.request_pickup();
        self.items[i].stack.count -= count;
        self.items[i].clear_pickup_request();
        split_offs.push(split);
    }

    /// Recompute every active item's cached light (after a world light update), so a
    /// dropped item brightens/dims when a nearby torch is placed or removed.
    pub fn refresh_lights(&mut self, world: &World) {
        for it in &mut self.items {
            let c = voxel_at(it.pos);
            it.skylight = world.combined_light6_at_world(c.x, c.y, c.z);
        }
    }

    /// Drain and return the active items resting in section `pos` — used to bundle
    /// them into that section's save record as it unloads.
    pub(super) fn take_items_in_section(&mut self, pos: SectionPos) -> Vec<DroppedItem> {
        let mut taken = Vec::new();
        let mut i = self.items.len();
        while i > 0 {
            i -= 1;
            if section_of(self.items[i].pos) == Some(pos) {
                taken.push(self.items.swap_remove(i));
            }
        }
        taken
    }

    /// Clone the active items grouped by owning section, for the periodic save
    /// flush (the items stay active; the clones persist with the section records so
    /// a crash can't lose their lifetimes). Drops outside the world vertical range
    /// (none in normal play) are dropped from the grouping.
    pub(super) fn items_by_section(&self) -> HashMap<SectionPos, Vec<DroppedItem>> {
        let mut map: HashMap<SectionPos, Vec<DroppedItem>> = HashMap::new();
        for it in &self.items {
            if let Some(pos) = section_of(it.pos) {
                map.entry(pos).or_default().push(it.clone());
            }
        }
        map
    }

    /// Append items read back from a chunk's save record (their paused lifetime
    /// timers resume now that the chunk is loaded again).
    pub(super) fn extend(&mut self, items: impl IntoIterator<Item = DroppedItem>) {
        self.items.extend(items);
    }
}

impl World {
    /// Add a dropped item to the active set (it must lie in a loaded chunk).
    pub fn spawn_item(&mut self, item: DroppedItem) {
        self.dropped_items.spawn(item);
    }

    /// The active dropped items, for the renderer's per-frame instance mapping.
    pub fn item_entities(&self) -> &[DroppedItem] {
        self.dropped_items.items()
    }

    /// Mutable access to the active item list, for tests that seed or trim it.
    #[cfg(test)]
    pub fn item_entities_mut(&mut self) -> &mut Vec<DroppedItem> {
        self.dropped_items.items_mut()
    }

    /// Mutable access to the active dropped items, so `Game` can borrow-split the
    /// drops (owned here) against the player inventory (owned by `Game`) to plan
    /// and absorb pickups without aliasing. The pickup-vs-inventory reconciliation
    /// itself stays in `Game`; `World` never sees the player inventory.
    pub fn dropped_items_mut(&mut self) -> &mut DroppedItems {
        &mut self.dropped_items
    }

    /// Per-frame physics for active items (gravity, collision, spin, pickup
    /// magnet). With a save attached, a drop over a not-yet-loaded chunk is frozen
    /// so it can't fall through missing terrain. Drives the owned [`DroppedItems`]
    /// against an immutable view of the rest of the world: the field is moved out
    /// so the `&mut DroppedItems` and `&World` borrows stay disjoint.
    pub fn tick_item_physics(&mut self, dt: f32, magnet_target: Vec3) {
        if self.dropped_items.is_empty() {
            return;
        }
        let freeze_unloaded = self.save.is_some();
        let mut drops = std::mem::take(&mut self.dropped_items);
        drops.tick_physics(self, dt, magnet_target, freeze_unloaded);
        self.dropped_items = drops;
    }

    /// Per fixed game-tick lifetime step: age each active item and despawn those
    /// past [`ITEM_LIFETIME_TICKS`]. With a save attached, an item over an unloaded
    /// chunk is paused. See [`DroppedItems::tick_lifetime`].
    pub fn tick_item_lifetime(&mut self) {
        if self.dropped_items.is_empty() {
            return;
        }
        let pause_unloaded = self.save.is_some();
        let mut drops = std::mem::take(&mut self.dropped_items);
        drops.tick_lifetime(self, pause_unloaded);
        self.dropped_items = drops;
    }

    /// Recompute every active item's cached skylight (after a world light update).
    pub fn refresh_item_lights(&mut self) {
        if self.dropped_items.is_empty() {
            return;
        }
        let mut drops = std::mem::take(&mut self.dropped_items);
        drops.refresh_lights(self);
        self.dropped_items = drops;
    }
}

/// Chunk (column) coordinates owning world position `pos`. Used for the
/// coarse "is the terrain under this drop loaded?" freeze check.
#[inline]
fn chunk_xz(pos: Vec3) -> (i32, i32) {
    ((pos.x.floor() as i32) >> 4, (pos.z.floor() as i32) >> 4)
}

/// The 16³ section owning world position `pos` (`None` if outside the world's
/// vertical range — not reachable in normal play).
#[inline]
fn section_of(pos: Vec3) -> Option<SectionPos> {
    SectionPos::from_world(
        pos.x.floor() as i32,
        pos.y.floor() as i32,
        pos.z.floor() as i32,
    )
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
        w.dropped_items_mut().request_pickups(player, |s| s.count);
        w.dropped_items_mut()
            .collect_requested_pickups(player, |s| {
                collected += s.count as u32;
                None
            });
        assert_eq!(collected, 0, "the pickup delay blocks collection");
        assert_eq!(w.item_entities().len(), 1);
        assert!(
            !w.item_entities()[0].pickup_requested,
            "delayed drops are not requested"
        );

        w.item_entities_mut()[0].ticks_lived = ITEM_PICKUP_DELAY_TICKS;
        w.dropped_items_mut().request_pickups(player, |s| s.count);
        w.dropped_items_mut()
            .collect_requested_pickups(player, |s| {
                collected += s.count as u32;
                None
            });
        assert_eq!(collected, 1, "collected once past the delay");
        assert!(w.item_entities().is_empty());
    }

    #[test]
    fn pickup_splits_off_only_the_part_that_fits() {
        let mut w = World::new(0, 0);
        let player = Vec3::new(0.5, 64.0, 0.5);
        let mut item = DroppedItem::new(player, ItemStack::new(ItemType::Dirt, 10), 1);
        item.ticks_lived = 1234; // past the delay, with a partly-elapsed despawn timer
        let origin_pos = item.pos;
        let origin_vel = item.vel; // the outward pop from `new`
        w.spawn_item(item);
        // The planned inventory can take only 6 of the 10.
        w.dropped_items_mut().request_pickups(player, |_| 6);

        // Two drops now: the reduced original and the requested split.
        assert_eq!(w.item_entities().len(), 2);
        let original = w
            .item_entities()
            .iter()
            .find(|d| !d.pickup_requested)
            .expect("original kept, despawn timer untouched");
        assert_eq!(
            original.stack.count, 4,
            "original reduced by the part that fit"
        );
        assert_eq!(
            original.ticks_lived, 1234,
            "original despawn timer untouched"
        );
        let split = w
            .item_entities()
            .iter()
            .find(|d| d.pickup_requested)
            .expect("split drop requested");
        assert_eq!(
            split.stack.count, 6,
            "split carries exactly the part that fit"
        );
        assert_eq!(split.stack.item, ItemType::Dirt);
        assert_eq!(split.ticks_lived, 1234, "split keeps the source lifetime");
        // Spawned exactly on the original, with its velocity — not just nearby.
        assert_eq!(
            split.pos, origin_pos,
            "split spawns exactly where the original is"
        );
        assert_eq!(
            split.vel, origin_vel,
            "split inherits the original's velocity"
        );
    }

    #[test]
    fn pickup_replans_existing_request_before_splitting_more() {
        let mut w = World::new(0, 0);
        let player = Vec3::new(0.5, 64.0, 0.5);
        let mut item = DroppedItem::new(player, ItemStack::new(ItemType::Dirt, 10), 1);
        item.ticks_lived = ITEM_PICKUP_DELAY_TICKS;
        w.spawn_item(item);

        let mut remaining = 6;
        w.dropped_items_mut().request_pickups(player, |s| {
            let count = remaining.min(s.count);
            remaining -= count;
            count
        });
        assert_eq!(w.item_entities().len(), 2);

        // Next tick has the same six slots still reserved by the already-requested
        // split. The planner must keep that request instead of splitting six more
        // from the original remainder.
        let mut remaining = 6;
        w.dropped_items_mut().request_pickups(player, |s| {
            let count = remaining.min(s.count);
            remaining -= count;
            count
        });

        assert_eq!(w.item_entities().len(), 2, "no duplicate split-off");
        let requested: u32 = w
            .item_entities()
            .iter()
            .filter(|d| d.pickup_requested)
            .map(|d| d.stack.count as u32)
            .sum();
        let unrequested: u32 = w
            .item_entities()
            .iter()
            .filter(|d| !d.pickup_requested)
            .map(|d| d.stack.count as u32)
            .sum();
        assert_eq!(requested, 6);
        assert_eq!(unrequested, 4);
    }

    #[test]
    fn a_split_drop_tracks_the_original_instead_of_drifting() {
        // Regression: the split used to spawn at rest while the original kept its
        // velocity, so once the magnet let go they fell on different arcs and
        // landed apart. Cloning the physics state keeps them locked together.
        let mut w = World::new(0, 0);
        let mut item = DroppedItem::new(
            Vec3::new(0.5, 80.0, 0.5),
            ItemStack::new(ItemType::Dirt, 10),
            7,
        );
        item.ticks_lived = ITEM_PICKUP_DELAY_TICKS;
        item.vel = Vec3::new(3.0, 0.0, 1.0); // sideways drift a position-only split would lose
        let player = Vec3::new(0.5, 80.0, 0.5);
        w.spawn_item(item);
        w.dropped_items_mut().request_pickups(player, |_| 6);
        assert_eq!(w.item_entities().len(), 2);

        // Free physics with the magnet target far away (no pull): both drops must
        // follow the same arc and stay in the exact same place.
        let far = Vec3::new(1000.0, 80.0, 0.5);
        for _ in 0..30 {
            w.tick_item_physics(1.0 / 60.0, far);
        }
        let p0 = w.item_entities()[0].pos;
        let p1 = w.item_entities()[1].pos;
        assert_eq!(p0, p1, "split and original stay co-located, not nearby");
    }

    #[test]
    fn pickup_leaves_a_drop_with_no_room() {
        let mut w = World::new(0, 0);
        let player = Vec3::new(0.5, 64.0, 0.5);
        let mut item = DroppedItem::new(player, ItemStack::new(ItemType::Dirt, 10), 1);
        item.ticks_lived = ITEM_PICKUP_DELAY_TICKS;
        w.spawn_item(item);
        w.dropped_items_mut().request_pickups(player, |_| 0);
        w.dropped_items_mut()
            .collect_requested_pickups(player, |_| None);
        assert_eq!(
            w.item_entities().len(),
            1,
            "a full inventory leaves the drop"
        );
        assert_eq!(w.item_entities()[0].stack.count, 10, "untouched");
        assert!(
            !w.item_entities()[0].pickup_requested,
            "unrequested drops are left alone"
        );
    }

    /// A drop that was not requested must not be magnetised: with the magnet off
    /// it falls under gravity rather than being sucked up to the player and pinned
    /// there with nowhere to go.
    #[test]
    fn magnet_skips_a_drop_that_was_not_requested() {
        let mut w = World::new(0, 0);
        let target = Vec3::new(0.5, 65.0, 0.5);
        let mut item = drop_at(0.5, 0.5);
        item.pos = Vec3::new(0.5, 64.5, 0.5); // 0.5 below the target, within attract range
        item.vel = Vec3::ZERO;
        item.ticks_lived = ITEM_PICKUP_DELAY_TICKS; // past the pickup delay
        w.spawn_item(item);

        let before_y = w.item_entities()[0].pos.y;
        w.tick_item_physics(1.0 / 60.0, target);
        let after_y = w.item_entities()[0].pos.y;
        assert!(
            after_y < before_y,
            "an unrequested drop should fall, not rise toward the player: {before_y} -> {after_y}"
        );
    }

    /// Once requested, the same drop is magnetised up toward the player target
    /// above it.
    #[test]
    fn magnet_pulls_a_requested_drop() {
        let mut w = World::new(0, 0);
        let target = Vec3::new(0.5, 65.0, 0.5);
        let mut item = drop_at(0.5, 0.5);
        item.pos = Vec3::new(0.5, 64.5, 0.5);
        item.vel = Vec3::ZERO;
        item.ticks_lived = ITEM_PICKUP_DELAY_TICKS;
        w.spawn_item(item);
        w.dropped_items_mut().request_pickups(target, |s| s.count);
        assert!(w.item_entities()[0].pickup_requested);

        let before_y = w.item_entities()[0].pos.y;
        w.tick_item_physics(1.0 / 60.0, target);
        let after_y = w.item_entities()[0].pos.y;
        assert!(
            after_y > before_y,
            "a requested drop should be pulled up toward the player: {before_y} -> {after_y}"
        );
    }

    #[test]
    fn unloading_a_section_harvests_only_its_items() {
        // take_items_in_section is what an unload uses to bundle a section's drops
        // into its save record (and so pause their timers). drop_at puts y=64 → cy 4.
        let mut w = World::new(0, 0);
        w.spawn_item(drop_at(2.5, 2.5)); // section (0, 4, 0)
        w.spawn_item(drop_at(20.5, 2.5)); // section (1, 4, 0)
        let taken = w
            .dropped_items_mut()
            .take_items_in_section(SectionPos::new(0, 4, 0));
        assert_eq!(taken.len(), 1, "only the (0,4,0) drop is harvested");
        assert_eq!(w.item_entities().len(), 1, "the (1,4,0) drop stays active");
        assert!(w.item_entities()[0].pos.x > 16.0);
    }

    #[test]
    fn items_group_by_owning_section_for_flush() {
        let mut w = World::new(0, 0);
        w.spawn_item(drop_at(2.5, 2.5)); // (0, 4, 0)
        w.spawn_item(drop_at(5.5, 9.5)); // (0, 4, 0)
        w.spawn_item(drop_at(20.5, 2.5)); // (1, 4, 0)
        let map = w.dropped_items_mut().items_by_section();
        assert_eq!(map[&SectionPos::new(0, 4, 0)].len(), 2);
        assert_eq!(map[&SectionPos::new(1, 4, 0)].len(), 1);
    }
}
