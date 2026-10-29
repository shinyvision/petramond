//! A fake host for the relay's tests. It copies the one engine rule the relay depends on: a block
//! that can hold a container but has none yet (`unopened`) reads as `None` and gets its container
//! on the first transfer into it (`insert_specs`), while a cell that cannot hold one refuses.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use mod_sdk::testing::{install_host, HostGuard};
use mod_sdk::*;

pub const RELAY: BlockId = BlockId(7);
pub const CHEST: BlockId = BlockId(8);
pub const STONE: &str = "petramond:stone";
pub const DIRT: &str = "petramond:dirt";
const STACK: u8 = 64;

#[derive(Default)]
pub struct State {
    pub blocks: HashMap<[i32; 3], BlockId>,
    pub containers: HashMap<ContainerAddress, Vec<Option<ItemStackData>>>,
    /// Chests whose container the engine has not created, with their slot count.
    pub unopened: HashMap<ContainerAddress, usize>,
    /// Containers that only admit this item.
    pub only: HashMap<ContainerAddress, &'static str>,
    pub rows: Vec<(BlockId, String)>,
    pub ticks: Vec<([i32; 3], u64)>,
    pub logs: Vec<String>,
    pub registered: Vec<&'static str>,
}

#[derive(Clone, Default)]
pub struct World(Rc<RefCell<State>>);

pub struct Session {
    pub world: World,
    _guard: HostGuard,
}

pub fn stack(item: &str, count: u8) -> ItemStackData {
    ItemStackData {
        item: item.into(),
        count,
        data: Vec::new(),
    }
}

impl Session {
    pub fn new() -> Self {
        let world = World::default();
        let host = world.clone();
        Self {
            _guard: install_host(move |call| host.answer(call)),
            world,
        }
    }
}

impl World {
    pub fn state(&self) -> std::cell::Ref<'_, State> {
        self.0.borrow()
    }

    pub fn row(&self, block: BlockId, json: &str) {
        self.0.borrow_mut().rows.push((block, json.into()));
    }

    pub fn place(&self, pos: [i32; 3], block: BlockId) {
        self.0.borrow_mut().blocks.insert(pos, block);
    }

    pub fn chest(&self, pos: [i32; 3], slots: usize) {
        self.place(pos, CHEST);
        self.0
            .borrow_mut()
            .containers
            .insert(pos.into(), vec![None; slots]);
    }

    pub fn unopened_chest(&self, pos: [i32; 3], slots: usize) {
        self.place(pos, CHEST);
        self.0.borrow_mut().unopened.insert(pos.into(), slots);
    }

    pub fn only(&self, pos: [i32; 3], item: &'static str) {
        self.0.borrow_mut().only.insert(pos.into(), item);
    }

    pub fn put(&self, pos: [i32; 3], slot: usize, item: &str, count: u8) {
        self.0
            .borrow_mut()
            .containers
            .get_mut(&pos.into())
            .expect("a container there")[slot] = Some(stack(item, count));
    }

    /// Items of `item` held at `pos`; zero where nothing is held yet.
    pub fn count(&self, pos: [i32; 3], item: &str) -> u32 {
        self.state().containers.get(&pos.into()).map_or(0, |slots| {
            slots
                .iter()
                .flatten()
                .filter(|s| s.item == item)
                .map(|s| u32::from(s.count))
                .sum()
        })
    }

    pub fn has_container(&self, pos: [i32; 3]) -> bool {
        self.state().containers.contains_key(&pos.into())
    }

    /// Takes the ticks scheduled so far.
    pub fn ticks(&self) -> Vec<([i32; 3], u64)> {
        std::mem::take(&mut self.0.borrow_mut().ticks)
    }

    pub fn logs(&self) -> Vec<String> {
        self.state().logs.clone()
    }

    fn answer(&self, call: &HostCall) -> HostRet {
        let mut s = self.0.borrow_mut();
        match call {
            HostCall::Core(CoreCall::Log { msg }) => {
                s.logs.push(msg.clone());
                HostRet::Unit
            }
            HostCall::Core(CoreCall::RegisterBlockBehavior { .. }) => {
                s.registered.push("behavior");
                HostRet::Unit
            }
            HostCall::Core(CoreCall::RegisterEventHandler { .. }) => {
                s.registered.push("handler");
                HostRet::Unit
            }
            HostCall::Block(BlockCall::GetBlocks { positions }) => {
                HostRet::Blocks(positions.iter().map(|p| s.blocks.get(p).copied()).collect())
            }
            HostCall::Block(BlockCall::ScheduleTicks { ticks }) => {
                s.ticks.extend(ticks.iter().copied());
                HostRet::Unit
            }
            HostCall::Registry(RegistryCall::BlocksWithData { .. }) => {
                HostRet::BlockDataRows(s.rows.iter().map(|(b, raw)| (*b, raw.clone())).collect())
            }
            HostCall::Registry(RegistryCall::BlockNames { blocks }) => HostRet::Names(
                blocks
                    .iter()
                    .map(|b| (*b == RELAY).then(|| "relay:relay".to_owned()))
                    .collect(),
            ),
            HostCall::Container(ContainerCall::ContainerGet { addresses }) => HostRet::Containers(
                addresses
                    .iter()
                    .map(|at| s.containers.get(at).cloned())
                    .collect(),
            ),
            HostCall::Container(ContainerCall::ContainerTransfer {
                from,
                slot,
                to,
                count,
            }) => HostRet::ItemStack(s.transfer(*from, *slot as usize, *to, *count)),
            other => panic!("the relay's fake host does not model {other:?}"),
        }
    }
}

impl State {
    fn transfer(
        &mut self,
        from: ContainerAddress,
        slot: usize,
        to: ContainerAddress,
        count: u8,
    ) -> Option<ItemStackData> {
        if !self.containers.contains_key(&to) {
            let slots = self.unopened.remove(&to)?;
            self.containers.insert(to, vec![None; slots]);
        }
        let taken = self.containers.get(&from)?.get(slot)?.clone()?;
        let asked = count.min(taken.count);
        let moved = if self.only.get(&to).is_some_and(|only| *only != taken.item) {
            0
        } else {
            asked - self.stow(to, &taken.item, asked)
        };
        if moved == 0 {
            return None;
        }
        let source = &mut self.containers.get_mut(&from)?[slot];
        *source = (taken.count > moved).then(|| stack(&taken.item, taken.count - moved));
        Some(stack(&taken.item, moved))
    }

    /// Puts `count` of `item` into `at`; returns what did not fit.
    fn stow(&mut self, at: ContainerAddress, item: &str, mut count: u8) -> u8 {
        let Some(slots) = self.containers.get_mut(&at) else {
            return count;
        };
        for held in slots.iter_mut() {
            if count == 0 {
                break;
            }
            match held {
                Some(s) if s.item == item && s.count < STACK => {
                    let put = count.min(STACK - s.count);
                    s.count += put;
                    count -= put;
                }
                None => {
                    let put = count.min(STACK);
                    *held = Some(stack(item, put));
                    count -= put;
                }
                _ => {}
            }
        }
        count
    }
}
