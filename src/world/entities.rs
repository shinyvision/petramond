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

use crate::entity::{DroppedItem, Motion};
use crate::mob::PlayerAnchor;
use crate::player::PlayerId;
use petramond_math::math::{voxel_at, IVec3, Vec3};
use petramond_world::chunk::SectionPos;
use petramond_world::item::ItemStack;

use super::store::World;

mod step;
mod sweep;
#[cfg(test)]
mod tests;

use step::{ChangedCells, StepCtx};
use sweep::SweepBodies;

/// Item entity lifetime: 6000 game ticks (5 minutes at 20 TPS). The timer only
/// advances while the holding chunk is loaded, and persists with the chunk, so an
/// item that has lived 3000 ticks still has 3000 ticks left after a reload.
pub const ITEM_LIFETIME_TICKS: u32 = 6000;

/// Ticks a freshly dropped/thrown item must live before it can be vacuumed up: 10
/// ticks (0.5 s at 20 TPS), so a just-thrown stack flies clear before the magnet
/// can pull it back.
pub const ITEM_PICKUP_DELAY_TICKS: u32 = 10;

/// Distance (centre to centre) below which two compatible dropped stacks
/// merge into one entity. Tight enough that a pile still reads as several
/// bobs, wide enough to catch a break burst's scattered pops.
pub const ITEM_MERGE_RADIUS: f32 = 1.0;

/// Merge cadence in game ticks (every 0.5 s at 20 TPS). Merging is invisible
/// bookkeeping, so it never needs to run per tick: the interval bounds its
/// worst-case cost to a twentieth of the per-tick budget share while piles
/// still form far faster than a player can notice the delay.
pub const ITEM_MERGE_INTERVAL_TICKS: u32 = 10;

/// One dropped-item environmental transform's presentation — ONE per
/// transformed entity, never per item in the stack. Returned from the physics
/// tick as a batch; the stage owner routes it onto the world-event channels.
pub struct ItemReactionFx {
    /// The item row's one-shot burst bundle id, if declared.
    pub burst: Option<u8>,
    /// The item row's one-shot sound, if declared.
    pub sound: Option<petramond_world::sound_registry::Sound>,
    pub pos: Vec3,
}

/// What a flying item struck.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ImpactTarget {
    Mob(u64),
    Player(PlayerId),
    /// The cell and the crossed face's normal (back toward the flight).
    Block {
        cell: IVec3,
        face: IVec3,
    },
}

/// A flying item's impact this tick: found by the sweep, RESOLVED by the
/// server (`projectile_hit` and its consequence), because what an impact
/// does is policy the world store must not carry. The item is already
/// seated at the impact, still in flight, when this is returned.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ItemImpact {
    /// The striking entity's stable id.
    pub id: u64,
    pub target: ImpactTarget,
    /// The impact point.
    pub point: Vec3,
    /// The velocity it arrived with.
    pub vel: Vec3,
}

/// Everything one item-physics tick reports back to the stage owner.
#[derive(Default)]
pub struct ItemStep {
    pub fx: Vec<ItemReactionFx>,
    pub impacts: Vec<ItemImpact>,
}

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
    /// Last assigned stable id (see [`DroppedItem::id`]). Session-scoped:
    /// reloaded drops get fresh ids, like everything entering the active set.
    next_id: u64,
}

impl DroppedItems {
    /// Stamp a fresh stable id onto a drop entering the active set.
    fn assign_id(&mut self, item: &mut DroppedItem) {
        self.next_id += 1;
        item.id = self.next_id;
    }

    /// Add a dropped item to the active set (it must lie in a loaded chunk)
    /// and answer its stable id.
    pub fn spawn(&mut self, mut item: DroppedItem) -> u64 {
        self.assign_id(&mut item);
        let id = item.id;
        self.items.push(item);
        id
    }

    /// The active dropped items, for the renderer's per-frame instance mapping.
    pub fn items(&self) -> &[DroppedItem] {
        &self.items
    }

    /// Mutable access to the active item list, for tests that seed or trim it.
    #[cfg(any(test, feature = "test-support"))]
    pub fn items_mut(&mut self) -> &mut Vec<DroppedItem> {
        &mut self.items
    }

    /// Whether there are no active drops (lets `World` skip the take/restore dance
    /// without exposing the backing list).
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Per-frame physics for active items: each item takes its own variant's
    /// step (`entities::step`), then the shared post-step — a light refresh
    /// on a cell crossing and the row-declared environmental reaction.
    ///
    /// A LOOSE stack falls, settles and magnets toward its REQUESTER's body
    /// centre (`anchors` carries every connected player's id, body centre
    /// and body). Only drops marked by [`request_pickups`](Self::request_pickups)
    /// are magnetised, so inventory capacity is planned before movement
    /// starts; a requester absent from the anchors (left this frame) simply
    /// exerts no pull until the next planner pass releases the mark.
    ///
    /// A FLIGHT moves by its own sweep: the first live body (the anchors'
    /// bodies, the world's mobs) or collidable block along this tick's
    /// motion stops it at the impact, reported in [`ItemStep::impacts`] for
    /// the stage owner to resolve. A LODGED item holds still and costs
    /// nothing until `changed` (the world's announced block changes since
    /// the last tick, `overflow` = positions lost, every anchor re-checked)
    /// names its anchor cell; the tick that cell loses its collision, it
    /// drops loose.
    ///
    /// When `freeze_unloaded` is set (a save is attached), a drop whose own
    /// cell or the cell under it has not ARRIVED (see
    /// [`terrain_under_drop_is_final`]) is frozen so it can't fall through
    /// missing terrain (in-memory worlds with no save always simulate,
    /// matching the test setups).
    ///
    /// Takes `world` (immutable) as a parameter; the caller must not be holding a
    /// borrow of these `DroppedItems` through the same `World`.
    pub fn tick_physics(
        &mut self,
        world: &World,
        dt: f32,
        anchors: &[PlayerAnchor],
        changed: &[IVec3],
        overflow: bool,
        freeze_unloaded: bool,
    ) -> ItemStep {
        let mut step = ItemStep::default();
        let any_flight = self
            .items
            .iter()
            .any(|it| matches!(it.motion, Motion::Flight(_)));
        let ctx = StepCtx {
            world,
            dt,
            anchors,
            changed: ChangedCells::new(changed, overflow),
            bodies: any_flight.then(|| SweepBodies::gather(world)),
        };
        for it in &mut self.items {
            if freeze_unloaded && !terrain_under_drop_is_final(world, it.pos) {
                continue;
            }
            let before = voxel_at(it.pos);
            if let Some(impact) = it.step(&ctx) {
                step.impacts.push(impact);
            }
            let after = voxel_at(it.pos);
            if before != after {
                it.skylight = world.skylight6_at_world(after.x, after.y, after.z);
                it.blocklight = petramond_world::light::BlockLight6::from_x2(
                    world.blocklight_rgb_at_world(after.x, after.y, after.z),
                );
            }
            // Row-declared environmental reaction (`DroppedReaction`): only
            // items whose row declares one pay the environment probe. The
            // whole stack transforms IN PLACE — count, motion, identity, age,
            // and pickup state stay; only the item kind changes — and it
            // naturally fires once (the result row no longer matches). The
            // probe is checked every ticked frame, not just on cell
            // crossings, so water flowing OVER a resting item still counts
            // as the item entering water. Stream-final gated: an in-flight
            // section reads as "not there yet", never a transient transform.
            if let Some(reaction) = it.stack.item.dropped_reaction() {
                let in_env = match reaction.environment {
                    petramond_world::item::ReactionEnvironment::Water => {
                        world.block_if_stream_final(after.x, after.y, after.z)
                            == Some(petramond_world::block::Block::Water)
                    }
                };
                if in_env {
                    it.stack.item = reaction.result;
                    step.fx.push(ItemReactionFx {
                        burst: reaction.burst,
                        sound: reaction.sound,
                        pos: it.pos,
                    });
                }
            }
        }
        step
    }

    /// The active item with stable id `id`. Linear: id lookups are a
    /// handful per tick (an impact, a query), never per item.
    pub fn get(&self, id: u64) -> Option<&DroppedItem> {
        self.items.iter().find(|it| it.id == id)
    }

    /// [`get`](Self::get), mutable — for the stage resolving an impact.
    pub fn get_mut(&mut self, id: u64) -> Option<&mut DroppedItem> {
        self.items.iter_mut().find(|it| it.id == id)
    }

    /// Remove the active item with stable id `id` — a flying item consumed
    /// by its impact. `false` = no such item.
    pub fn remove(&mut self, id: u64) -> bool {
        match self.items.iter().position(|it| it.id == id) {
            Some(at) => {
                self.items.swap_remove(at);
                true
            }
            None => false,
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

    /// Per fixed game-tick pickup planning for ONE player (`requester`).
    /// Eligible drops are those past the pickup delay, inside the player's
    /// attract radius, and not already reserved by ANOTHER player (a drop is
    /// requested by at most one player at a time — first come per tick, in
    /// session order; the marks are re-evaluated every tick). For each
    /// candidate, `request` returns how many items are reserved by the
    /// inventory simulation: `0` leaves the drop alone, the full count
    /// requests the whole entity, and a partial count splits that many items
    /// into a requested entity while leaving the remainder unrequested.
    ///
    /// The requester's already-requested drops are planned first. That keeps a
    /// split-off stack from being duplicated every tick while it is flying
    /// toward the player.
    pub fn request_pickups(
        &mut self,
        requester: PlayerId,
        player_pos: Vec3,
        mut request: impl FnMut(ItemStack) -> u8,
    ) {
        let was_requested: Vec<Option<PlayerId>> =
            self.items.iter().map(|d| d.pickup_requested).collect();
        let mut split_offs = Vec::new();

        for (i, &requested) in was_requested.iter().enumerate() {
            if requested != Some(requester) {
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
                self.apply_pickup_request(i, requester, count, &mut split_offs);
            }
        }

        for (i, &requested) in was_requested.iter().enumerate() {
            // Another player's reservation is respected whole; their own
            // planner pass re-evaluates it on their turn.
            if requested.is_some() || !self.pickup_request_candidate(i, player_pos) {
                continue;
            }
            let count = request(self.items[i].stack).min(self.items[i].stack.count);
            if count > 0 {
                self.apply_pickup_request(i, requester, count, &mut split_offs);
            }
        }

        self.items.extend(split_offs);
    }

    /// Per fixed game-tick pickup absorption for ONE player: only drops
    /// requested BY `requester` can be collected. `deposit` returns any
    /// leftover that did not fit; a leftover drop has its request cleared so
    /// the next planner pass can decide what to do.
    pub fn collect_requested_pickups(
        &mut self,
        requester: PlayerId,
        player_pos: Vec3,
        mut deposit: impl FnMut(ItemStack) -> Option<ItemStack>,
    ) {
        let mut i = self.items.len();
        while i > 0 {
            i -= 1;
            if self.items[i].pickup_requested != Some(requester) {
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

    /// Release every reservation whose owner fails `still_valid` — the leave/
    /// death sweep run once per tick before the planner passes, so a gone (or
    /// dead, hence no longer planning) requester's drops return to the pool
    /// the very next tick instead of staying reserved forever.
    pub fn release_requests_not_from(&mut self, still_valid: impl Fn(PlayerId) -> bool) {
        for item in &mut self.items {
            if item.pickup_requested.is_some_and(|by| !still_valid(by)) {
                item.clear_pickup_request();
            }
        }
    }

    /// Merge compatible dropped stacks within [`ITEM_MERGE_RADIUS`] of each
    /// other into one entity each. Runs on the fixed tick (cadence-gated by
    /// the caller) so scattered break bursts and mob-drop piles collapse into
    /// single stacks instead of rendering (and shadowing) as a swarm of
    /// one-item entities.
    ///
    /// Compatibility is exactly inventory stacking (`ItemStack::can_stack_with`
    /// — same item AND same instance-data variant), and merges respect the
    /// item's max stack size: a pile of 40+40 dirt becomes 64 + a 16 remainder,
    /// never an oversized stack.
    ///
    /// A drop with an active pickup reservation never takes part — it is
    /// magnet-flying to its requester, whose inventory has already reserved
    /// those items; absorbing it would strand the reservation.
    ///
    /// Performance: a spatial hash by voxel cell means the pass is O(items)
    /// plus O(matches), never O(n²) — only drops that share a cell or border
    /// one are distance-tested. The survivor keeps its identity (id, pose,
    /// velocity); the absorbed stack's count folds in, keeping the OLDER
    /// despawn timer so merging never shortens a pile's life. Removals happen
    /// once at the end (flag + compact), so buckets stay valid mid-pass.
    pub fn merge_nearby(&mut self) {
        if self.items.len() < 2 {
            return;
        }
        let mut cells: HashMap<(i32, i32, i32), Vec<u32>> = HashMap::new();
        for (i, it) in self.items.iter().enumerate() {
            let c = voxel_at(it.pos);
            cells.entry((c.x, c.y, c.z)).or_default().push(i as u32);
        }
        let mut consumed = vec![false; self.items.len()];
        // Only LOOSE stacks merge: a lodged item stays where it struck, and
        // one in the air is nobody's pile yet.
        let loose = |it: &DroppedItem| matches!(it.motion, Motion::Loose);
        for i in 0..self.items.len() {
            if consumed[i] || self.items[i].pickup_requested.is_some() || !loose(&self.items[i]) {
                continue;
            }
            let origin = voxel_at(self.items[i].pos);
            // The radius fits inside the 3×3×3 cell neighbourhood around the
            // survivor's cell (worst case: adjacent cells touching corner-wise).
            let near = |cells: &HashMap<(i32, i32, i32), Vec<u32>>| {
                let mut out = Vec::new();
                for dx in -1..=1 {
                    for dy in -1..=1 {
                        for dz in -1..=1 {
                            if let Some(bucket) =
                                cells.get(&(origin.x + dx, origin.y + dy, origin.z + dz))
                            {
                                out.extend(
                                    bucket
                                        .iter()
                                        .copied()
                                        .map(|j| j as usize)
                                        .filter(|&j| j > i),
                                );
                            }
                        }
                    }
                }
                out
            };
            for j in near(&cells) {
                if consumed[j] || self.items[j].pickup_requested.is_some() || !loose(&self.items[j])
                {
                    continue;
                }
                let d = self.items[i].pos - self.items[j].pos;
                let (stackable, space, b_count, b_ticks) = {
                    let (a, b) = (&self.items[i], &self.items[j]);
                    (
                        a.stack.can_stack_with(&b.stack)
                            && d.length_squared() <= ITEM_MERGE_RADIUS * ITEM_MERGE_RADIUS,
                        a.stack.space_left(),
                        b.stack.count,
                        b.ticks_lived,
                    )
                };
                if !stackable {
                    continue;
                }
                if space == 0 {
                    break;
                }
                let take = space.min(b_count);
                self.items[i].stack.count += take;
                self.items[i].ticks_lived = self.items[i].ticks_lived.min(b_ticks);
                if take == b_count {
                    consumed[j] = true;
                } else {
                    self.items[j].stack.count -= take;
                }
            }
        }
        let mut w = 0;
        for (r, gone) in consumed.iter().enumerate() {
            if !gone {
                self.items.swap(w, r);
                w += 1;
            }
        }
        self.items.truncate(w);
    }

    fn pickup_request_candidate(&self, i: usize, player_pos: Vec3) -> bool {
        let item = &self.items[i];
        item.ticks_lived >= ITEM_PICKUP_DELAY_TICKS
            && item.collectable()
            && !item.stack.is_empty()
            && item.within_attract(player_pos)
    }

    fn apply_pickup_request(
        &mut self,
        i: usize,
        requester: PlayerId,
        count: u8,
        split_offs: &mut Vec<DroppedItem>,
    ) {
        debug_assert!(count > 0);
        let stack_count = self.items[i].stack.count;
        if count >= stack_count {
            self.items[i].request_pickup(requester);
            return;
        }

        // Clone the full physics state so the requested part starts exactly where
        // the source stack is. The remainder is left unrequested and therefore
        // will not be pulled by the magnet. The split-off is a NEW entity and
        // gets its own stable id (the source keeps its id with fewer items).
        let mut split = self.items[i].clone();
        self.assign_id(&mut split);
        split.stack.count = count;
        split.request_pickup(requester);
        self.items[i].stack.count -= count;
        self.items[i].clear_pickup_request();
        split_offs.push(split);
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
    /// timers resume now that the chunk is loaded again). Each gets a fresh
    /// stable id — ids are session-scoped, never persisted.
    pub(super) fn extend(&mut self, items: impl IntoIterator<Item = DroppedItem>) {
        for mut item in items {
            self.assign_id(&mut item);
            self.items.push(item);
        }
    }
}

impl World {
    /// Add a dropped item to the active set (it must lie in a loaded chunk).
    pub fn spawn_item(&mut self, item: DroppedItem) -> u64 {
        self.dropped_items.spawn(item)
    }

    /// The active dropped items, for the renderer's per-frame instance mapping.
    pub fn item_entities(&self) -> &[DroppedItem] {
        self.dropped_items.items()
    }

    /// Mutable access to the active item list, for tests that seed or trim it.
    #[cfg(any(test, feature = "test-support"))]
    pub fn item_entities_mut(&mut self) -> &mut Vec<DroppedItem> {
        self.dropped_items.items_mut()
    }

    /// The active dropped items, for reads addressed by stable id.
    pub fn dropped_items(&self) -> &DroppedItems {
        &self.dropped_items
    }

    /// Mutable access to the active dropped items, so `Game` can borrow-split the
    /// drops (owned here) against the player inventory (owned by `Game`) to plan
    /// and absorb pickups without aliasing. The pickup-vs-inventory reconciliation
    /// itself stays in `Game`; `World` never sees the player inventory.
    pub fn dropped_items_mut(&mut self) -> &mut DroppedItems {
        &mut self.dropped_items
    }

    /// Per-frame physics for active items (gravity, collision, spin, pickup
    /// magnet toward each requested drop's own requester, flight sweeps
    /// against the anchors' bodies and the mobs). With a save attached, a drop
    /// over a not-yet-loaded chunk is frozen so it can't fall through missing
    /// terrain. Drives the owned `DroppedItems` against an immutable view of
    /// the rest of the world: the field is moved out so the
    /// `&mut DroppedItems` and `&World` borrows stay disjoint.
    pub fn tick_item_physics(&mut self, dt: f32, anchors: &[PlayerAnchor]) -> ItemStep {
        // Drained whether or not anything is listening, so a drop-free world
        // never grows the buffer to its overflow.
        let (changed, overflow) = self.take_collision_changes();
        if self.dropped_items.is_empty() {
            return ItemStep::default();
        }
        let freeze_unloaded = self.save.is_some();
        let mut drops = std::mem::take(&mut self.dropped_items);
        let step = drops.tick_physics(self, dt, anchors, &changed, overflow, freeze_unloaded);
        self.dropped_items = drops;
        step
    }

    /// Per fixed game-tick lifetime step: age each active item and despawn those
    /// past `ITEM_LIFETIME_TICKS`. With a save attached, an item over an unloaded
    /// chunk is paused. See `DroppedItems::tick_lifetime`.
    pub fn tick_item_lifetime(&mut self) {
        if self.dropped_items.is_empty() {
            return;
        }
        let pause_unloaded = self.save.is_some();
        let mut drops = std::mem::take(&mut self.dropped_items);
        drops.tick_lifetime(self, pause_unloaded);
        self.dropped_items = drops;
    }
}

/// Chunk (column) coordinates owning world position `pos`. Used for the
/// coarse "is the terrain under this drop loaded?" freeze check.
#[inline]
fn chunk_xz(pos: Vec3) -> (i32, i32) {
    ((pos.x.floor() as i32) >> 4, (pos.z.floor() as i32) >> 4)
}

/// Whether the terrain a drop at `pos` rests on or falls through has ARRIVED:
/// its own cell and the cell under it read final (a loaded section, or an
/// absent one whose generated summary proves it uniform), and it is above the
/// world floor. Otherwise the drop holds still this tick.
///
/// A loaded COLUMN is not enough: the world is cubic, so a deep section can
/// be out of the vertical window while the sections above it are loaded. A
/// drop simulated against that absent floor reads air, falls through it and
/// is out of the world a second later — a corpse pile spilled where a player
/// died on floor the server had not regenerated yet.
fn terrain_under_drop_is_final(world: &World, pos: Vec3) -> bool {
    let c = voxel_at(pos);
    c.y >= petramond_world::chunk::WORLD_MIN_Y
        && world.physics_cell_final_at(c.x, c.y, c.z)
        && world.physics_cell_final_at(c.x, c.y - 1, c.z)
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
