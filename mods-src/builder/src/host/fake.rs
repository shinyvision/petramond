mod answers;
mod nav;
pub mod rows;

use std::cell::{Ref, RefCell, RefMut};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use crate::fx::HashMap;
use crate::host::prelude::*;

use rows::{BlockRow, ItemRow, AIR};

pub struct Schematic {
    pub title: String,
    pub size: [i32; 3],
    pub sections: Vec<SchematicCellsData>,
}

pub struct Mob {
    pub kind: MobId,
    pub pos: [f64; 3],
    pub yaw: f32,
    pub health: f32,
    pub on_ground: bool,
    pub tags: BTreeMap<String, MobTagValue>,
    pub anims: BTreeSet<String>,
    pub held: (Option<String>, Option<String>),
    pub drawn: usize,
    pub driven: Option<[f32; 3]>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Deed {
    Placed([i32; 3], String),
    Dug([i32; 3]),
    Used([i32; 3]),
    Held([i32; 3], bool),
    Dropped(String, u8),
    Ghost(String, bool),
    Opened(String),
    Chose(String),
    Positioned(String),
    Sound(String),
    Burst(String),
    Parts([i32; 3], u32),
    Logged(String),
}

pub struct State {
    pub now: u64,
    pub blocks: HashMap<[i32; 3], BlockId>,
    pub unloaded: Vec<([i32; 3], [i32; 3])>,
    pub changes: Vec<[i32; 3]>,
    pub block_rows: Vec<BlockRow>,
    pub item_rows: Vec<ItemRow>,
    pub mob_rows: Vec<&'static str>,
    pub groups: HashMap<[i32; 3], [i32; 3]>,
    pub mobs: BTreeMap<u64, Mob>,
    next_mob: u64,
    pub containers: HashMap<ContainerAddress, Vec<Option<ItemStackData>>>,
    pub schematics: HashMap<SchematicId, Schematic>,
    pub lookups: HashMap<SchematicId, SchematicLookup>,
    pub kv: BTreeMap<String, Vec<u8>>,
    pub players: Vec<(PlayerId, [f64; 3])>,
    pub acting: Option<PlayerId>,
    pub held: HashMap<PlayerId, ItemStackData>,
    pub identities: HashMap<PlayerId, PlayerIdentityData>,
    pub viewers: Vec<GuiViewerData>,
    pub route_budget: Option<u32>,
    pub deeds: Vec<Deed>,
}

impl State {
    pub fn row(&self, block: BlockId) -> &BlockRow {
        &self.block_rows[usize::from(block.0)]
    }

    pub fn loaded(&self, cell: [i32; 3]) -> bool {
        !self
            .unloaded
            .iter()
            .any(|(min, max)| (0..3).all(|i| (min[i]..=max[i]).contains(&cell[i])))
    }

    pub fn block_at(&self, cell: [i32; 3]) -> Option<BlockId> {
        self.loaded(cell)
            .then(|| self.blocks.get(&cell).copied().unwrap_or(AIR))
    }

    pub fn set(&mut self, cell: [i32; 3], block: BlockId) {
        if block == AIR {
            self.blocks.remove(&cell);
        } else {
            self.blocks.insert(cell, block);
        }
        self.changes.push(cell);
    }

    pub fn resolve_block(&self, name: &str) -> Option<BlockId> {
        self.block_rows
            .iter()
            .position(|row| row.name == name)
            .map(|i| BlockId(i as u16))
    }

    pub fn item_row(&self, name: &str) -> Option<&ItemRow> {
        self.item_rows.iter().find(|row| row.name == name)
    }

    pub fn item_of(&self, block: BlockId) -> Option<&'static str> {
        let item = self.row(block).info.item?;
        Some(self.item_rows[usize::from(item.0)].name)
    }

    fn spawn(&mut self, kind: MobId, pos: [f64; 3], yaw: f32) -> u64 {
        self.next_mob += 1;
        let id = self.next_mob;
        self.mobs.insert(
            id,
            Mob {
                kind,
                pos,
                yaw,
                health: rows::GOLEM_HEALTH,
                on_ground: true,
                tags: BTreeMap::new(),
                anims: BTreeSet::new(),
                held: (None, None),
                drawn: 0,
                driven: None,
            },
        );
        self.containers
            .insert(ContainerAddress::Mob(id), vec![None; rows::GOLEM_SLOTS]);
        id
    }

    pub fn stow(&mut self, at: ContainerAddress, mut stack: ItemStackData) -> u8 {
        let most = self
            .item_row(&stack.item)
            .map_or(64, |row| row.info.max_stack);
        let Some(slots) = self.containers.get_mut(&at) else {
            return stack.count;
        };
        for slot in slots.iter_mut().flatten() {
            if slot.item == stack.item && slot.data == stack.data && slot.count < most {
                let moved = stack.count.min(most - slot.count);
                slot.count += moved;
                stack.count -= moved;
            }
        }
        for slot in slots.iter_mut().filter(|s| s.is_none()) {
            if stack.count == 0 {
                break;
            }
            let moved = stack.count.min(most);
            *slot = Some(ItemStackData {
                count: moved,
                ..stack.clone()
            });
            stack.count -= moved;
        }
        stack.count
    }

    pub fn spend(&mut self, at: ContainerAddress, item: &str, count: u8) -> bool {
        let Some(slots) = self.containers.get_mut(&at) else {
            return false;
        };
        let held: u32 = slots
            .iter()
            .flatten()
            .filter(|s| s.item == item)
            .map(|s| u32::from(s.count))
            .sum();
        if held < u32::from(count) {
            return false;
        }
        let mut left = count;
        for slot in slots.iter_mut() {
            let Some(stack) = slot.as_mut().filter(|s| s.item == item) else {
                continue;
            };
            let taken = left.min(stack.count);
            stack.count -= taken;
            left -= taken;
            if stack.count == 0 {
                *slot = None;
            }
        }
        true
    }
}

pub struct Fake {
    state: RefCell<State>,
}

impl Default for Fake {
    fn default() -> Self {
        Self::new()
    }
}

impl Fake {
    pub fn new() -> Self {
        Self {
            state: RefCell::new(State {
                now: 1,
                blocks: HashMap::default(),
                unloaded: Vec::new(),
                changes: Vec::new(),
                block_rows: rows::blocks(),
                item_rows: rows::items(),
                mob_rows: rows::mobs(),
                groups: HashMap::default(),
                mobs: BTreeMap::new(),
                next_mob: 100,
                containers: HashMap::default(),
                schematics: HashMap::default(),
                lookups: HashMap::default(),
                kv: BTreeMap::new(),
                players: Vec::new(),
                acting: None,
                held: HashMap::default(),
                identities: HashMap::default(),
                viewers: Vec::new(),
                route_budget: None,
                deeds: Vec::new(),
            }),
        }
    }

    pub fn state(&self) -> Ref<'_, State> {
        self.state.borrow()
    }

    pub fn state_mut(&self) -> RefMut<'_, State> {
        self.state.borrow_mut()
    }

    pub fn install(self: &Rc<Self>) -> Installed {
        let world = Rc::clone(self);
        Installed {
            _sdk_host: mod_sdk::testing::install_host(move |call| bridge::answer(&world, call)),
            _host: super::installed::install(Rc::clone(self) as Rc<dyn super::Host>),
        }
    }

    pub fn set(&self, cell: [i32; 3], block: BlockId) {
        self.state_mut().set(cell, block);
    }

    pub fn fill(&self, min: [i32; 3], max: [i32; 3], block: BlockId) {
        let mut state = self.state_mut();
        for x in min[0]..=max[0] {
            for y in min[1]..=max[1] {
                for z in min[2]..=max[2] {
                    state.set([x, y, z], block);
                }
            }
        }
    }

    pub fn block(&self, cell: [i32; 3]) -> BlockId {
        self.state().blocks.get(&cell).copied().unwrap_or(AIR)
    }

    pub fn unload(&self, min: [i32; 3], max: [i32; 3]) {
        self.state_mut().unloaded.push((min, max));
    }

    pub fn set_now(&self, now: u64) {
        self.state_mut().now = now;
    }

    pub fn golem_at(&self, cell: [i32; 3]) -> u64 {
        self.state_mut()
            .spawn(rows::GOLEM_KIND, crate::geometry::feet_of(cell), 0.0)
    }

    pub fn chest(&self, cell: [i32; 3], slots: usize) {
        let mut state = self.state_mut();
        state.set(cell, rows::CHEST);
        state
            .containers
            .insert(ContainerAddress::Block(cell), vec![None; slots]);
    }

    pub fn give(&self, at: ContainerAddress, item: &str, count: u8) {
        let left = self.state_mut().stow(at, stack(item, count));
        assert_eq!(left, 0, "{at:?} has no room for {count}x {item}");
    }

    pub fn put(&self, at: ContainerAddress, slot: usize, stack: Option<ItemStackData>) {
        self.state_mut()
            .containers
            .get_mut(&at)
            .expect("a container there")[slot] = stack;
    }

    pub fn container(&self, at: ContainerAddress) -> Vec<Option<ItemStackData>> {
        self.state()
            .containers
            .get(&at)
            .cloned()
            .unwrap_or_default()
    }

    pub fn count(&self, at: ContainerAddress, item: &str) -> u32 {
        self.container(at)
            .iter()
            .flatten()
            .filter(|s| s.item == item)
            .map(|s| u32::from(s.count))
            .sum()
    }

    pub fn schematic(
        &self,
        asset: SchematicId,
        title: &str,
        size: [i32; 3],
        cells: &[([i32; 3], &str)],
        per: usize,
    ) {
        let sections = cells
            .chunks(per.max(1))
            .map(|chunk| {
                let mut palette: Vec<BlockRecord> = Vec::new();
                let mut out = Vec::new();
                for (pos, name) in chunk {
                    let record = record(name);
                    let index = palette
                        .iter()
                        .position(|r| *r == record)
                        .unwrap_or_else(|| {
                            palette.push(record);
                            palette.len() - 1
                        });
                    out.push((*pos, index as u16));
                }
                SchematicCellsData {
                    cells: out,
                    palette,
                }
            })
            .collect();
        self.state_mut().schematics.insert(
            asset,
            Schematic {
                title: title.into(),
                size,
                sections,
            },
        );
    }

    pub fn schematic_lookup(&self, asset: SchematicId, lookup: SchematicLookup) {
        self.state_mut().lookups.insert(asset, lookup);
    }

    pub fn route_budget(&self, budget: Option<u32>) {
        self.state_mut().route_budget = budget;
    }

    pub fn tag(&self, mob: u64, key: &str) -> Option<MobTagValue> {
        self.state().mobs.get(&mob)?.tags.get(key).cloned()
    }

    pub fn mob_pos(&self, mob: u64) -> Option<[f64; 3]> {
        Some(self.state().mobs.get(&mob)?.pos)
    }

    pub fn deeds(&self) -> Vec<Deed> {
        self.state().deeds.clone()
    }

    pub fn foothold(&self, cell: [i32; 3]) -> bool {
        nav::Ground::new(&self.state(), &[]).foothold(cell)
    }
}

pub fn stack(item: &str, count: u8) -> ItemStackData {
    ItemStackData {
        item: item.into(),
        count,
        data: Vec::new(),
    }
}

pub fn record(name: &str) -> BlockRecord {
    BlockRecord {
        block: name.into(),
        state: Vec::new(),
        refs: Vec::new(),
        data: Vec::new(),
    }
}

pub struct Installed {
    _sdk_host: mod_sdk::testing::HostGuard,
    _host: super::installed::Installed,
}

mod bridge {

    use mod_sdk::{HostCall, HostRet};

    use super::{Deed, Fake};

    pub fn answer(world: &Fake, call: &HostCall) -> HostRet {
        let mut state = world.state_mut();
        match call {
            HostCall::Kv(mod_sdk::KvCall::WorldKvGet { key }) => {
                HostRet::Bytes(state.kv.get(key).cloned())
            }
            HostCall::Kv(mod_sdk::KvCall::WorldKvSet { key, value }) => {
                state.kv.insert(key.clone(), value.clone());
                HostRet::Unit
            }
            HostCall::Kv(mod_sdk::KvCall::WorldKvDelete { key }) => {
                HostRet::Bool(state.kv.remove(key).is_some())
            }
            HostCall::Block(mod_sdk::BlockCall::BlockChangesSince { since }) => {
                HostRet::BlockChanges(super::answers::changes_since(&state, *since))
            }
            HostCall::Core(mod_sdk::CoreCall::Log { msg }) => {
                state.deeds.push(Deed::Logged(msg.clone()));
                HostRet::Unit
            }
            other => panic!("the builder called the SDK directly, around its host seam: {other:?}"),
        }
    }
}
