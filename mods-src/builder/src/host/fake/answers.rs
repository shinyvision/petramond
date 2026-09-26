//! The fake world's answer to each host call.

use crate::geometry::offset;
use crate::host::prelude::{
    ActionRefusal, BlockChanges, BlockId, BlockInfoData, BlockRecord, ContainerAddress,
    DigProgress, DrawFrame, DrawPrim, EntityRef, Flood, GuiViewerData, ItemId, ItemInfoData,
    ItemStackData, MobId, MobSnapshot, MobTagLookup, MobTagValue, ModelGroupData, ParticleTexture,
    PlaceRequest, PlayerId, PlayerIdentityData, RecordPlan, RecordStatus, Route,
    SchematicCellsData, SchematicGhostData, SchematicId, SchematicInfoData, SchematicLookup,
};
use crate::host::Host;

use super::nav::Ground;
use super::rows::AIR;
use super::{stack, Deed, Fake, State};

impl State {
    fn mob_of(&self, actor: EntityRef) -> Option<&super::Mob> {
        match actor {
            EntityRef::Mob(id) => self.mobs.get(&id),
            EntityRef::Player(_) => None,
        }
    }

    fn snapshot(&self, id: u64) -> Option<MobSnapshot> {
        let mob = self.mobs.get(&id)?;
        Some(MobSnapshot {
            index: 0,
            kind: mob.kind,
            pos: mob.pos,
            health: mob.health,
            id,
            yaw: mob.yaw,
            pitch: 0.0,
            roll: 0.0,
            vel: [0.0; 3],
            on_ground: mob.on_ground,
            moving: false,
            half_width: crate::content::GOLEM_HALF_WIDTH,
            height: 1.5,
            half_length: crate::content::GOLEM_HALF_WIDTH,
            entombed: false,
            conditions: Vec::new(),
        })
    }

    fn holds_items(&self, cell: [i32; 3]) -> bool {
        self.containers
            .get(&ContainerAddress::Block(cell))
            .is_some_and(|slots| slots.iter().any(Option::is_some))
    }

    /// What building `record` costs from nothing, and the cells it fills.
    fn plan(&self, record: &BlockRecord) -> RecordPlan {
        let Some(block) = self.resolve_block(&record.block) else {
            return RecordPlan::Unsupported {
                reason: format!("Unknown block {}", record.block),
            };
        };
        let row = self.row(block);
        if block == AIR {
            return RecordPlan::Air;
        }
        if row.info.fluid.is_some() {
            return RecordPlan::Unsupported {
                reason: "Fluids cannot be built".into(),
            };
        }
        match self.item_of(block) {
            Some(item) => RecordPlan::Unit {
                cost: vec![stack(item, 1)],
                footprint: row.footprint.clone(),
            },
            None => RecordPlan::Unsupported {
                reason: format!("Nothing places {}", record.block),
            },
        }
    }

    fn status(&self, pos: [i32; 3], record: &BlockRecord) -> RecordStatus {
        let Some(here) = self.block_at(pos) else {
            return RecordStatus::Unloaded;
        };
        let footprint = match self.plan(record) {
            RecordPlan::Air => {
                return if self.row(here).info.replaceable {
                    RecordStatus::Satisfied
                } else {
                    self.clear(pos, here)
                };
            }
            RecordPlan::Unit { footprint, .. } => footprint,
            RecordPlan::Member { anchor } => return RecordStatus::Pending { anchor },
            RecordPlan::Unsupported { reason } => return RecordStatus::Unsupported { reason },
        };
        let block = self.resolve_block(&record.block).unwrap_or(AIR);
        let cells: Vec<[i32; 3]> = footprint.iter().map(|o| offset(pos, *o)).collect();
        let mut built = true;
        for cell in &cells {
            let Some(there) = self.block_at(*cell) else {
                return RecordStatus::Unloaded;
            };
            if there == block {
                continue;
            }
            built = false;
            if !self.row(there).info.replaceable {
                return self.clear(*cell, there);
            }
        }
        if built {
            return RecordStatus::Satisfied;
        }
        match self.plan(record) {
            RecordPlan::Unit { cost, .. } => RecordStatus::Place { missing: cost },
            _ => RecordStatus::Satisfied,
        }
    }

    fn clear(&self, at: [i32; 3], block: BlockId) -> RecordStatus {
        RecordStatus::Clear {
            at,
            block,
            footprint: vec![at],
            holds_items: self.holds_items(at),
        }
    }

    /// Whether `actor` may place `record` at `pos` from `feet`.
    fn place_check(
        &self,
        actor: EntityRef,
        feet: [f64; 3],
        pos: [i32; 3],
        record: &BlockRecord,
        pay: bool,
    ) -> PlaceRequest {
        if self.mob_of(actor).is_none() {
            return PlaceRequest::Refused(ActionRefusal::NoActor);
        }
        let Some(block) = self.resolve_block(&record.block) else {
            return PlaceRequest::Refused(ActionRefusal::Unsupported);
        };
        let Some(here) = self.block_at(pos) else {
            return PlaceRequest::Refused(ActionRefusal::Unloaded);
        };
        if here == block {
            return PlaceRequest::Satisfied;
        }
        if !self.row(here).info.replaceable {
            return PlaceRequest::Refused(ActionRefusal::Obstructed);
        }
        if let Err(refusal) = Ground::new(self, &[]).aim(feet, pos, Some(record)) {
            return PlaceRequest::Refused(refusal);
        }
        let paid = match (pay, self.item_of(block), actor) {
            (false, ..) => true,
            (true, Some(item), EntityRef::Mob(id)) => self
                .containers
                .get(&ContainerAddress::Mob(id))
                .is_some_and(|slots| slots.iter().flatten().any(|s| s.item == item)),
            _ => false,
        };
        if !paid {
            return PlaceRequest::Refused(ActionRefusal::MissingItems);
        }
        PlaceRequest::Queued
    }
}

impl Host for Fake {
    fn get_block(&self, pos: [i32; 3]) -> Option<BlockId> {
        self.state().block_at(pos)
    }

    fn get_blocks(&self, positions: Vec<[i32; 3]>) -> Vec<Option<BlockId>> {
        let state = self.state();
        positions.into_iter().map(|p| state.block_at(p)).collect()
    }

    fn is_loaded(&self, pos: [i32; 3]) -> bool {
        self.state().loaded(pos)
    }

    fn find_blocks(
        &self,
        min: [i32; 3],
        max: [i32; 3],
        blocks: Vec<BlockId>,
    ) -> Option<Vec<[i32; 3]>> {
        let state = self.state();
        let mut found = Vec::new();
        for x in min[0]..=max[0] {
            for y in min[1]..=max[1] {
                for z in min[2]..=max[2] {
                    if blocks.contains(&state.block_at([x, y, z])?) {
                        found.push([x, y, z]);
                    }
                }
            }
        }
        Some(found)
    }

    fn block_model_group(&self, pos: [i32; 3]) -> Option<ModelGroupData> {
        let base = *self.state().groups.get(&pos)?;
        Some(ModelGroupData {
            base,
            facing: crate::host::prelude::Facing::North,
        })
    }

    fn set_model_parts(&self, pos: [i32; 3], parts: u32, _tint: Option<[u8; 3]>) -> bool {
        self.state_mut().deeds.push(Deed::Parts(pos, parts));
        true
    }

    fn resolve_block(&self, name: &str) -> Option<BlockId> {
        self.state().resolve_block(name)
    }

    fn resolve_item(&self, name: &str) -> Option<ItemId> {
        self.state()
            .item_rows
            .iter()
            .position(|row| row.name == name)
            .map(|i| ItemId(i as u16))
    }

    fn resolve_mob(&self, key: &str) -> Option<MobId> {
        self.state()
            .mob_rows
            .iter()
            .position(|row| *row == key)
            .map(|i| MobId(i as u8))
    }

    fn block_info(&self, block: BlockId) -> Option<BlockInfoData> {
        let state = self.state();
        state
            .block_rows
            .get(usize::from(block.0))
            .map(|row| row.info.clone())
    }

    fn block_infos(&self, blocks: Vec<BlockId>) -> Vec<Option<BlockInfoData>> {
        blocks.into_iter().map(|b| self.block_info(b)).collect()
    }

    fn block_names(&self, blocks: Vec<BlockId>) -> Vec<Option<String>> {
        let state = self.state();
        blocks
            .into_iter()
            .map(|b| {
                state
                    .block_rows
                    .get(usize::from(b.0))
                    .map(|row| row.name.to_owned())
            })
            .collect()
    }

    fn item_info(&self, item: &str) -> Option<ItemInfoData> {
        self.state().item_row(item).map(|row| row.info.clone())
    }

    fn item_names(&self, items: Vec<ItemId>) -> Vec<Option<String>> {
        let state = self.state();
        items
            .into_iter()
            .map(|i| {
                state
                    .item_rows
                    .get(usize::from(i.0))
                    .map(|row| row.name.to_owned())
            })
            .collect()
    }

    fn blocks_by_tag(&self, tag: &str) -> Vec<BlockId> {
        let state = self.state();
        (0..state.block_rows.len())
            .map(|i| BlockId(i as u16))
            .filter(|b| state.row(*b).tags.iter().any(|t| t == tag))
            .collect()
    }

    fn blocks_with_data(&self, key: &str) -> Vec<(BlockId, String)> {
        let state = self.state();
        (0..state.block_rows.len())
            .map(|i| BlockId(i as u16))
            .filter_map(|b| {
                let (_, value) = state.row(b).data.iter().find(|(k, _)| k == key)?;
                Some((b, value.clone()))
            })
            .collect()
    }

    fn block_record_plans(&self, records: Vec<BlockRecord>) -> Vec<RecordPlan> {
        let state = self.state();
        records.iter().map(|r| state.plan(r)).collect()
    }

    fn block_record_statuses(&self, cells: Vec<([i32; 3], BlockRecord)>) -> Vec<RecordStatus> {
        let state = self.state();
        cells.iter().map(|(pos, r)| state.status(*pos, r)).collect()
    }

    fn actor_aims(
        &self,
        actor: EntityRef,
        from: Vec<[f64; 3]>,
        pos: [i32; 3],
        record: Option<BlockRecord>,
    ) -> Vec<Result<[f64; 3], ActionRefusal>> {
        let state = self.state();
        if state.mob_of(actor).is_none() {
            return vec![Err(ActionRefusal::NoActor); from.len()];
        }
        let ground = Ground::new(&state, &[]);
        from.into_iter()
            .map(|feet| ground.aim(feet, pos, record.as_ref()))
            .collect()
    }

    fn actor_place_check(
        &self,
        actor: EntityRef,
        from: [f64; 3],
        pos: [i32; 3],
        record: BlockRecord,
        pay: bool,
    ) -> PlaceRequest {
        self.state().place_check(actor, from, pos, &record, pay)
    }

    fn actor_place(
        &self,
        actor: EntityRef,
        pos: [i32; 3],
        record: BlockRecord,
        pay: bool,
    ) -> PlaceRequest {
        let mut state = self.state_mut();
        let Some(feet) = state.mob_of(actor).map(|m| m.pos) else {
            return PlaceRequest::Refused(ActionRefusal::NoActor);
        };
        let answer = state.place_check(actor, feet, pos, &record, pay);
        if answer != PlaceRequest::Queued {
            return answer;
        }
        let block = state.resolve_block(&record.block).unwrap_or(AIR);
        if let (true, Some(item), EntityRef::Mob(id)) = (pay, state.item_of(block), actor) {
            state.spend(ContainerAddress::Mob(id), item, 1);
        }
        for cell in state.row(block).footprint.clone() {
            state.set(offset(pos, cell), block);
        }
        state.deeds.push(Deed::Placed(pos, record.block));
        PlaceRequest::Queued
    }

    fn actor_dig(
        &self,
        actor: EntityRef,
        pos: [i32; 3],
        _tool_slot: Option<u32>,
        collect: bool,
    ) -> DigProgress {
        let mut state = self.state_mut();
        let Some(feet) = state.mob_of(actor).map(|m| m.pos) else {
            return DigProgress::Refused(ActionRefusal::NoActor);
        };
        let Some(block) = state.block_at(pos) else {
            return DigProgress::Refused(ActionRefusal::Unloaded);
        };
        if state.row(block).info.hardness < 0.0 {
            return DigProgress::Refused(ActionRefusal::Unbreakable);
        }
        if let Err(refusal) = Ground::new(&state, &[]).aim(feet, pos, None) {
            return DigProgress::Refused(refusal);
        }
        state.set(pos, AIR);
        if let (true, Some(item), EntityRef::Mob(id)) = (collect, state.item_of(block), actor) {
            state.stow(ContainerAddress::Mob(id), stack(item, 1));
        }
        state.deeds.push(Deed::Dug(pos));
        DigProgress::Breaking
    }

    fn actor_interact(&self, actor: EntityRef, pos: [i32; 3]) -> bool {
        let mut state = self.state_mut();
        let usable = state.mob_of(actor).is_some()
            && state
                .block_at(pos)
                .is_some_and(|b| state.row(b).info.interaction.is_some());
        if usable {
            state.deeds.push(Deed::Used(pos));
        }
        usable
    }

    fn footholds(&self, _key: &str, cells: Vec<[i32; 3]>) -> Vec<bool> {
        let state = self.state();
        let ground = Ground::new(&state, &[]);
        cells.into_iter().map(|c| ground.foothold(c)).collect()
    }

    fn path_probe(
        &self,
        _key: &str,
        from: [i32; 3],
        to: [i32; 3],
        blocked: Vec<[i32; 3]>,
        max_nodes: u32,
    ) -> Option<Route> {
        let mut state = self.state_mut();
        match &mut state.route_budget {
            Some(0) => return None,
            Some(left) => *left -= 1,
            None => {}
        }
        Some(Ground::new(&state, &blocked).route(from, to, max_nodes))
    }

    fn walk_region(
        &self,
        _key: &str,
        from: [i32; 3],
        min: [i32; 3],
        max: [i32; 3],
        blocked: Vec<[i32; 3]>,
        toward: bool,
        max_nodes: u32,
    ) -> Flood {
        let mut state = self.state_mut();
        match &mut state.route_budget {
            Some(0) => return Flood::Deferred,
            Some(left) => *left -= 1,
            None => {}
        }
        match Ground::new(&state, &blocked).flood(from, (min, max), toward, max_nodes) {
            Some(reached) => Flood::Reached(reached),
            None => Flood::Exceeded,
        }
    }

    fn mob_can_reach(&self, mob_id: u64, cell: [i32; 3]) -> bool {
        let state = self.state();
        let Some(mob) = state.mobs.get(&mob_id) else {
            return false;
        };
        let from = crate::geometry::cell_of(mob.pos);
        Ground::new(&state, &[]).route(from, cell, u32::MAX) == Route::Open
    }

    fn spawn_mob(&self, key: &str, pos: [f64; 3], yaw: f32) -> Option<u64> {
        let kind = self.resolve_mob(key)?;
        Some(self.state_mut().spawn(kind, pos, yaw))
    }

    fn despawn_mob(&self, mob_id: u64) -> bool {
        let mut state = self.state_mut();
        state.containers.remove(&ContainerAddress::Mob(mob_id));
        state.mobs.remove(&mob_id).is_some()
    }

    fn mob_info(&self, mob_id: u64) -> Option<MobSnapshot> {
        self.state().snapshot(mob_id)
    }

    fn mobs_with_tag(&self, key: &str, value: Option<MobTagValue>) -> Vec<MobSnapshot> {
        let state = self.state();
        state
            .mobs
            .iter()
            .filter(|(_, mob)| match (&value, mob.tags.get(key)) {
                (_, None) => false,
                (None, Some(_)) => true,
                (Some(want), Some(has)) => want == has,
            })
            .filter_map(|(id, _)| state.snapshot(*id))
            .collect()
    }

    fn mob_tag_get(&self, mob_id: u64, key: &str) -> MobTagLookup {
        match self.state().mobs.get(&mob_id) {
            None => MobTagLookup::MissingMob,
            Some(mob) => match mob.tags.get(key) {
                Some(value) => MobTagLookup::Value(value.clone()),
                None => MobTagLookup::Absent,
            },
        }
    }

    fn mob_tag_set(&self, mob_id: u64, key: &str, value: MobTagValue) -> bool {
        match self.state_mut().mobs.get_mut(&mob_id) {
            Some(mob) => {
                mob.tags.insert(key.into(), value);
                true
            }
            None => false,
        }
    }

    fn mob_tag_delete(&self, mob_id: u64, key: &str) -> bool {
        self.state_mut()
            .mobs
            .get_mut(&mob_id)
            .is_some_and(|mob| mob.tags.remove(key).is_some())
    }

    fn mob_kinematic(&self, mob_id: u64, pos: [f64; 3], yaw: f32, _pitch: f32, _roll: f32) -> bool {
        match self.state_mut().mobs.get_mut(&mob_id) {
            Some(mob) => {
                mob.pos = pos;
                mob.yaw = yaw;
                true
            }
            None => false,
        }
    }

    fn mob_drive(&self, mob_id: u64, vel: [f32; 2], _yaw: Option<f32>) -> bool {
        self.mob_drive_velocity(mob_id, [vel[0], 0.0, vel[1]], None)
    }

    fn mob_drive_velocity(&self, mob_id: u64, vel: [f32; 3], _yaw: Option<f32>) -> bool {
        match self.state_mut().mobs.get_mut(&mob_id) {
            Some(mob) => {
                mob.driven = Some(vel);
                true
            }
            None => false,
        }
    }

    fn mob_step(&self, mob_id: u64, vel: [f32; 2]) -> bool {
        self.mob_drive_velocity(mob_id, [vel[0], 0.0, vel[1]], None)
    }

    fn mob_anim_set(&self, mob_id: u64, anim: &str, active: bool) -> bool {
        match self.state_mut().mobs.get_mut(&mob_id) {
            Some(mob) => {
                if active {
                    mob.anims.insert(anim.into());
                } else {
                    mob.anims.remove(anim);
                }
                true
            }
            None => false,
        }
    }

    fn mob_held_display(&self, mob_id: u64, main: Option<String>, off: Option<String>) -> bool {
        match self.state_mut().mobs.get_mut(&mob_id) {
            Some(mob) => {
                mob.held = (main, off);
                true
            }
            None => false,
        }
    }

    fn set_mob_draw(&self, mob_id: u64, _frame: DrawFrame, prims: Vec<DrawPrim>) -> bool {
        match self.state_mut().mobs.get_mut(&mob_id) {
            Some(mob) => {
                mob.drawn = prims.len();
                true
            }
            None => false,
        }
    }

    fn container_get(&self, at: ContainerAddress) -> Option<Vec<Option<ItemStackData>>> {
        let state = self.state();
        if let ContainerAddress::Block(cell) = at {
            state.block_at(cell)?;
        }
        state.containers.get(&at).cloned()
    }

    fn container_get_many(
        &self,
        addresses: Vec<ContainerAddress>,
    ) -> Vec<Option<Vec<Option<ItemStackData>>>> {
        addresses
            .into_iter()
            .map(|at| self.container_get(at))
            .collect()
    }

    fn container_set(&self, at: ContainerAddress, slots: Vec<(u32, Option<ItemStackData>)>) -> bool {
        let mut state = self.state_mut();
        let Some(held) = state.containers.get_mut(&at) else {
            return false;
        };
        for (slot, stack) in slots {
            if let Some(held) = held.get_mut(slot as usize) {
                *held = stack;
            }
        }
        true
    }

    fn container_transfer(
        &self,
        from: ContainerAddress,
        slot: u32,
        to: ContainerAddress,
        count: u8,
    ) -> Option<ItemStackData> {
        let mut state = self.state_mut();
        let taken = state.containers.get(&from)?.get(slot as usize)?.clone()?;
        if !state.containers.contains_key(&to) {
            return None;
        }
        let asked = count.min(taken.count);
        let left = state.stow(
            to,
            ItemStackData {
                count: asked,
                ..taken.clone()
            },
        );
        let moved = asked - left;
        if moved == 0 {
            return None;
        }
        let source = &mut state.containers.get_mut(&from)?[slot as usize];
        *source = (taken.count > moved).then(|| ItemStackData {
            count: taken.count - moved,
            ..taken.clone()
        });
        Some(ItemStackData {
            count: moved,
            ..taken
        })
    }

    fn container_hold(&self, at: ContainerAddress, actor: EntityRef, open: bool) -> bool {
        let mut state = self.state_mut();
        let (ContainerAddress::Block(cell), true) = (at, state.mob_of(actor).is_some()) else {
            return false;
        };
        state.deeds.push(Deed::Held(cell, open));
        true
    }

    fn schematic_info(&self, asset: SchematicId) -> SchematicLookup {
        let state = self.state();
        if let Some(lookup) = state.lookups.get(&asset) {
            return lookup.clone();
        }
        match state.schematics.get(&asset) {
            Some(schematic) => SchematicLookup::Ready(SchematicInfoData {
                title: schematic.title.clone(),
                size: schematic.size,
                cells: schematic.sections.iter().map(|s| s.cells.len() as u64).sum(),
                sections: schematic.sections.len() as u32,
            }),
            None => SchematicLookup::Missing,
        }
    }

    fn schematic_cells(
        &self,
        asset: SchematicId,
        section: u32,
        turns: u8,
    ) -> Option<SchematicCellsData> {
        let state = self.state();
        let schematic = state.schematics.get(&asset)?;
        let mut cells = schematic.sections.get(section as usize)?.clone();
        let mut size = schematic.size;
        for _ in 0..turns % 4 {
            for (pos, _) in &mut cells.cells {
                *pos = [size[2] - 1 - pos[2], pos[1], pos[0]];
            }
            size = [size[2], size[1], size[0]];
        }
        Some(cells)
    }

    fn schematic_ghost_set(&self, key: &str, ghost: Option<SchematicGhostData>) -> bool {
        self.state_mut()
            .deeds
            .push(Deed::Ghost(key.into(), ghost.is_some()));
        true
    }

    fn schematic_choose(&self, _player: PlayerId, tag: &str) -> bool {
        self.state_mut().deeds.push(Deed::Chose(tag.into()));
        true
    }

    fn schematic_position(
        &self,
        _player: PlayerId,
        tag: &str,
        _asset: SchematicId,
        _origin: Option<[i32; 3]>,
        _turns: u8,
    ) -> bool {
        self.state_mut().deeds.push(Deed::Positioned(tag.into()));
        true
    }

    fn players(&self) -> Vec<(PlayerId, [f64; 3])> {
        self.state().players.clone()
    }

    fn acting_player(&self) -> Option<PlayerId> {
        self.state().acting
    }

    fn player_held(&self, player: PlayerId) -> Option<ItemStackData> {
        self.state().held.get(&player).cloned()
    }

    fn player_identity(&self, player: PlayerId) -> Option<PlayerIdentityData> {
        self.state().identities.get(&player).cloned()
    }

    fn gui_viewers(&self) -> Vec<GuiViewerData> {
        self.state().viewers.clone()
    }

    fn gui_open(&self, kind_key: &str, _at: Option<ContainerAddress>) -> bool {
        self.state_mut().deeds.push(Deed::Opened(kind_key.into()));
        true
    }

    fn emitter_burst_of(
        &self,
        key: &str,
        _pos: [f64; 3],
        _intensity: f32,
        _direction: Option<[f32; 3]>,
        _texture: Option<ParticleTexture>,
    ) -> bool {
        self.state_mut().deeds.push(Deed::Burst(key.into()));
        true
    }

    fn sound_play_at(&self, key: &str, _pos: [f64; 3], _volume: f32, _pitch: f32) -> u64 {
        let mut state = self.state_mut();
        state.deeds.push(Deed::Sound(key.into()));
        state.deeds.len() as u64
    }

    fn spawn_item_data(&self, item: &str, count: u8, _pos: [f64; 3], _data: &[(&str, &[u8])]) -> bool {
        self.state_mut()
            .deeds
            .push(Deed::Dropped(item.into(), count));
        true
    }

    fn current_tick(&self) -> u64 {
        self.state().now
    }

    fn rng_u64(&self, _stream_key: &str) -> u64 {
        0x5EED_5EED_5EED_5EED
    }

    fn world_kv_get(&self, key: &str) -> Option<Vec<u8>> {
        self.state().kv.get(key).cloned()
    }

    fn world_kv_set(&self, key: &str, value: Vec<u8>) {
        self.state_mut().kv.insert(key.into(), value);
    }
}

/// The cells the change log holds after `since`, as the bridge answers the
/// SDK's cursor.
pub(super) fn changes_since(state: &State, since: Option<u64>) -> BlockChanges {
    let next = state.changes.len() as u64;
    let from = since.map_or(next, |s| s.min(next)) as usize;
    BlockChanges {
        next,
        lost: false,
        cells: state.changes[from..].to_vec(),
    }
}
