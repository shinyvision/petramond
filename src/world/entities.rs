use crate::world::ServerWorld;

use crate::entity::{DroppedItem, Motion};
use crate::mob::PlayerAnchor;
use crate::world::session::PlayerId;
use petramond_math::math::{IVec3, Vec3};
use petramond_world::chunk::SectionPos;
use petramond_world::item::ItemStack;

mod influence;
mod step;
mod sweep;
#[cfg(test)]
mod tests;

use step::{ChangedCells, StepCtx};
use sweep::SweepBodies;

pub const ITEM_LIFETIME_TICKS: u32 = 6000;

pub const ITEM_PICKUP_DELAY_TICKS: u32 = 10;

pub const ITEM_MERGE_RADIUS: f32 = 1.0;

pub const ITEM_MERGE_INTERVAL_TICKS: u32 = 10;

pub struct ItemReactionFx {
    pub burst: Option<u8>,
    pub sound: Option<petramond_world::sound_registry::Sound>,
    pub pos: petramond_math::world_pos::WorldPos,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ImpactTarget {
    Mob(u64),
    Player(PlayerId),
    Block { cell: IVec3, face: IVec3 },
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ItemImpact {
    pub id: u64,
    pub target: ImpactTarget,
    pub point: petramond_math::world_pos::WorldPos,
    pub vel: Vec3,
}

#[derive(Default)]
pub struct ItemStep {
    pub fx: Vec<ItemReactionFx>,
    pub impacts: Vec<ItemImpact>,
}

#[derive(Default)]
pub struct DroppedItems {
    items: Vec<DroppedItem>,
    next_id: u64,
    change_seq: u64,
}

impl DroppedItems {
    fn assign_id(&mut self, item: &mut DroppedItem) {
        self.next_id += 1;
        item.id = self.next_id;
    }

    pub fn spawn(&mut self, mut item: DroppedItem) -> u64 {
        self.assign_id(&mut item);
        let id = item.id;
        self.items.push(item);
        id
    }

    pub fn items(&self) -> &[DroppedItem] {
        &self.items
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn items_mut(&mut self) -> &mut Vec<DroppedItem> {
        &mut self.items
    }

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
    /// resumes its flight with a small horizontal drift along its heading.
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
        world: &ServerWorld,
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
            let before = it.pos.block();
            if let Some(impact) = it.step(&ctx) {
                step.impacts.push(impact);
            }
            let after = it.pos.block();
            if before != after {
                it.skylight = world.data.skylight6_at_world(after.x, after.y, after.z);
                it.blocklight = petramond_world::light::BlockLight6::from_x2(
                    world
                        .data
                        .blocklight_rgb_at_world(after.x, after.y, after.z),
                );
            }
            let immersion = world
                .block_if_stream_final(after.x, after.y, after.z)
                .and_then(|_| world.data.fluid_at_point(it.pos));
            if immersion.is_some_and(|i| i.fluid.contact.destroys_items) {
                it.ticks_lived = ITEM_LIFETIME_TICKS;
            }
            if let Some(reaction) = it.stack.item.dropped_reaction() {
                if immersion.is_some_and(|i| i.fluid.block == reaction.fluid) {
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

    pub fn get(&self, id: u64) -> Option<&DroppedItem> {
        self.items.iter().find(|it| it.id == id)
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut DroppedItem> {
        self.items.iter_mut().find(|it| it.id == id)
    }

    pub fn remove(&mut self, id: u64) -> bool {
        match self.items.iter().position(|it| it.id == id) {
            Some(at) => {
                self.items.swap_remove(at);
                true
            }
            None => false,
        }
    }

    pub fn tick_lifetime(&mut self, world: &ServerWorld, pause_unloaded: bool) {
        let mut i = self.items.len();
        while i > 0 {
            i -= 1;
            if pause_unloaded {
                let (cx, cz) = chunk_xz(self.items[i].pos);
                if !world.data.chunk_loaded(cx, cz) {
                    continue;
                }
            }
            let lived = self.items[i].ticks_lived.saturating_add(1);
            self.items[i].ticks_lived = lived;
            if lived
                >= self.items[i]
                    .lifetime_ticks()
                    .unwrap_or(ITEM_LIFETIME_TICKS)
            {
                self.items.swap_remove(i);
            }
        }
    }

    pub fn request_pickups(
        &mut self,
        requester: PlayerId,
        player_pos: petramond_math::world_pos::WorldPos,
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

    pub fn collect_requested_pickups(
        &mut self,
        requester: PlayerId,
        player_pos: petramond_math::world_pos::WorldPos,
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

    pub fn release_requests_not_from(&mut self, still_valid: impl Fn(PlayerId) -> bool) {
        for item in &mut self.items {
            if item.pickup_requested.is_some_and(|by| !still_valid(by)) {
                item.clear_pickup_request();
            }
        }
    }

    pub fn merge_nearby(&mut self) {
        if self.items.len() < 2 {
            return;
        }
        let mut cells: rustc_hash::FxHashMap<(i32, i32, i32), Vec<u32>> = Default::default();
        for (i, it) in self.items.iter().enumerate() {
            let c = it.pos.block();
            cells.entry((c.x, c.y, c.z)).or_default().push(i as u32);
        }
        let mut consumed = vec![false; self.items.len()];
        let loose = |it: &DroppedItem| matches!(it.motion, Motion::Loose);
        for i in 0..self.items.len() {
            if consumed[i] || self.items[i].pickup_requested.is_some() || !loose(&self.items[i]) {
                continue;
            }
            let origin = self.items[i].pos.block();
            let near = |cells: &rustc_hash::FxHashMap<(i32, i32, i32), Vec<u32>>| {
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
                            && (a.lifetime_ticks().is_none() || a.ticks_lived == b.ticks_lived)
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

    fn pickup_request_candidate(
        &self,
        i: usize,
        player_pos: petramond_math::world_pos::WorldPos,
    ) -> bool {
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

        let mut split = self.items[i].clone();
        self.assign_id(&mut split);
        split.stack.count = count;
        split.request_pickup(requester);
        self.items[i].stack.count -= count;
        self.items[i].clear_pickup_request();
        split_offs.push(split);
    }

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

    pub(super) fn items_by_section(&self) -> rustc_hash::FxHashMap<SectionPos, Vec<DroppedItem>> {
        let mut map: rustc_hash::FxHashMap<SectionPos, Vec<DroppedItem>> = Default::default();
        for it in &self.items {
            if let Some(pos) = section_of(it.pos) {
                map.entry(pos).or_default().push(it.clone());
            }
        }
        map
    }

    pub(super) fn extend(&mut self, items: impl IntoIterator<Item = DroppedItem>) {
        for mut item in items {
            self.assign_id(&mut item);
            self.items.push(item);
        }
    }
}

impl ServerWorld {
    pub fn spawn_item(&mut self, item: DroppedItem) -> u64 {
        self.side.entities.dropped_items.spawn(item)
    }

    pub fn item_entities(&self) -> &[DroppedItem] {
        self.side.entities.dropped_items.items()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn item_entities_mut(&mut self) -> &mut Vec<DroppedItem> {
        self.side.entities.dropped_items.items_mut()
    }

    pub fn dropped_items(&self) -> &DroppedItems {
        &self.side.entities.dropped_items
    }

    pub fn dropped_items_mut(&mut self) -> &mut DroppedItems {
        &mut self.side.entities.dropped_items
    }

    pub fn tick_item_physics(&mut self, dt: f32, anchors: &[PlayerAnchor]) -> ItemStep {
        if self.side.entities.dropped_items.is_empty() {
            self.side.entities.dropped_items.change_seq = self.changes_end();
            return ItemStep::default();
        }
        let (next, changed, overflow) =
            self.changes_since(self.side.entities.dropped_items.change_seq);
        self.side.entities.dropped_items.change_seq = next;
        let freeze_unloaded = self.side.save.is_some();
        let mut drops = std::mem::take(&mut self.side.entities.dropped_items);
        let step = drops.tick_physics(self, dt, anchors, &changed, overflow, freeze_unloaded);
        self.side.entities.dropped_items = drops;
        step
    }

    pub fn tick_item_lifetime(&mut self) {
        if self.side.entities.dropped_items.is_empty() {
            return;
        }
        let pause_unloaded = self.side.save.is_some();
        let mut drops = std::mem::take(&mut self.side.entities.dropped_items);
        drops.tick_lifetime(self, pause_unloaded);
        self.side.entities.dropped_items = drops;
    }
}

#[inline]
fn chunk_xz(pos: petramond_math::world_pos::WorldPos) -> (i32, i32) {
    ((pos.x.floor() as i32) >> 4, (pos.z.floor() as i32) >> 4)
}

fn terrain_under_drop_is_final(
    world: &ServerWorld,
    pos: petramond_math::world_pos::WorldPos,
) -> bool {
    let c = pos.block();
    c.y >= petramond_world::chunk::WORLD_MIN_Y
        && world.physics_cell_final_at(c.x, c.y, c.z)
        && world.physics_cell_final_at(c.x, c.y - 1, c.z)
}

#[inline]
fn section_of(pos: petramond_math::world_pos::WorldPos) -> Option<SectionPos> {
    SectionPos::from_world(
        pos.x.floor() as i32,
        pos.y.floor() as i32,
        pos.z.floor() as i32,
    )
}
