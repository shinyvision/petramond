use std::cell::RefCell;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::rc::Rc;

use mod_api::{
    BlockCall, BlockId, ContainerAddress, ContainerCall, CoreCall, HostCall, HostRet,
    ItemStackData, KvCall, MobTagLookup, MobTagValue, RuntimeSide, TagCall,
};

type NativeHost = Box<dyn FnMut(&HostCall) -> HostRet>;

thread_local! {
    static HOST: RefCell<Option<NativeHost>> = const { RefCell::new(None) };
}

pub(crate) fn answer_natively(call: &HostCall) -> Option<HostRet> {
    HOST.with(|slot| {
        let mut slot = slot
            .try_borrow_mut()
            .expect("a native test host made an SDK host call while answering one");
        slot.as_mut().map(|host| host(call))
    })
}

#[must_use = "the host is uninstalled when the guard drops"]
pub struct HostGuard {
    previous: Option<NativeHost>,
    _thread_bound: PhantomData<*const ()>,
}

impl Drop for HostGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        let _ = HOST.try_with(|slot| *slot.borrow_mut() = previous);
    }
}

pub fn install_host(host: impl FnMut(&HostCall) -> HostRet + 'static) -> HostGuard {
    let previous = HOST.with(|slot| slot.borrow_mut().replace(Box::new(host)));
    HostGuard {
        previous,
        _thread_bound: PhantomData,
    }
}

pub fn with_host<R>(host: impl FnMut(&HostCall) -> HostRet + 'static, f: impl FnOnce() -> R) -> R {
    let _guard = install_host(host);
    f()
}

type Fallback = Box<dyn FnMut(&HostCall) -> Option<HostRet>>;

#[derive(Clone, Default)]
pub struct MockHost {
    state: Rc<RefCell<MockState>>,
    fallback: Rc<RefCell<Option<Fallback>>>,
}

#[derive(Default)]
struct MockState {
    tick: u64,
    blocks: HashMap<[i32; 3], BlockId>,
    loaded: Option<Vec<[i32; 3]>>,
    world_kv: HashMap<String, Vec<u8>>,
    cell_kv: HashMap<([i32; 3], String), Vec<u8>>,
    mobs: HashMap<u64, HashMap<String, MobTagValue>>,
    containers: HashMap<ContainerAddress, Vec<Option<ItemStackData>>>,
    rng_queue: Vec<u64>,
    rng_streams: HashMap<String, u64>,
    logs: Vec<String>,
    calls: Vec<HostCall>,
}

impl MockHost {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn install(&self) -> HostGuard {
        let host = self.clone();
        install_host(move |call| host.answer(call))
    }

    pub fn run<R>(&self, f: impl FnOnce() -> R) -> R {
        let _guard = self.install();
        f()
    }

    pub fn on(&self, handler: impl FnMut(&HostCall) -> Option<HostRet> + 'static) {
        *self.fallback.borrow_mut() = Some(Box::new(handler));
    }

    pub fn set_tick(&self, tick: u64) {
        self.state.borrow_mut().tick = tick;
    }

    pub fn set_block(&self, pos: [i32; 3], block: BlockId) {
        self.state.borrow_mut().blocks.insert(pos, block);
    }

    pub fn block(&self, pos: [i32; 3]) -> Option<BlockId> {
        self.state.borrow().blocks.get(&pos).copied()
    }

    pub fn set_loaded(&self, cells: impl IntoIterator<Item = [i32; 3]>) {
        self.state.borrow_mut().loaded = Some(cells.into_iter().collect());
    }

    pub fn set_world_kv(&self, key: &str, value: &[u8]) {
        let mut state = self.state.borrow_mut();
        state.world_kv.insert(key.to_owned(), value.to_vec());
    }

    pub fn world_kv(&self, key: &str) -> Option<Vec<u8>> {
        self.state.borrow().world_kv.get(key).cloned()
    }

    pub fn set_cell_kv(&self, pos: [i32; 3], key: &str, value: &[u8]) {
        let mut state = self.state.borrow_mut();
        state.cell_kv.insert((pos, key.to_owned()), value.to_vec());
    }

    pub fn cell_kv(&self, pos: [i32; 3], key: &str) -> Option<Vec<u8>> {
        self.state
            .borrow()
            .cell_kv
            .get(&(pos, key.to_owned()))
            .cloned()
    }

    pub fn add_mob(&self, id: u64) {
        self.state.borrow_mut().mobs.entry(id).or_default();
    }

    pub fn mob_tag(&self, id: u64, key: &str) -> Option<MobTagValue> {
        self.state.borrow().mobs.get(&id)?.get(key).cloned()
    }

    pub fn set_container(&self, at: ContainerAddress, slots: Vec<Option<ItemStackData>>) {
        self.state.borrow_mut().containers.insert(at, slots);
    }

    pub fn container(&self, at: ContainerAddress) -> Option<Vec<Option<ItemStackData>>> {
        self.state.borrow().containers.get(&at).cloned()
    }

    pub fn queue_rng(&self, values: impl IntoIterator<Item = u64>) {
        self.state.borrow_mut().rng_queue.extend(values);
    }

    pub fn logs(&self) -> Vec<String> {
        self.state.borrow().logs.clone()
    }

    pub fn calls(&self) -> Vec<HostCall> {
        self.state.borrow().calls.clone()
    }

    fn answer(&self, call: &HostCall) -> HostRet {
        self.state.borrow_mut().calls.push(call.clone());
        if let Some(handler) = self.fallback.borrow_mut().as_mut() {
            if let Some(ret) = handler(call) {
                return ret;
            }
        }
        self.state.borrow_mut().model(call).unwrap_or_else(|| {
            panic!("MockHost does not model {call:?}; answer it with MockHost::on")
        })
    }
}

impl MockState {
    fn loaded(&self, pos: &[i32; 3]) -> bool {
        self.loaded.as_ref().is_none_or(|cells| cells.contains(pos))
    }

    fn read_block(&self, pos: &[i32; 3]) -> Option<BlockId> {
        if !self.loaded(pos) {
            return None;
        }
        Some(self.blocks.get(pos).copied().unwrap_or(BlockId(0)))
    }

    fn write_block(&mut self, pos: [i32; 3], block: BlockId) -> bool {
        if !self.loaded(&pos) {
            return false;
        }
        self.blocks.insert(pos, block);
        true
    }

    fn rng_next(&mut self, key: &str) -> u64 {
        if !self.rng_queue.is_empty() {
            return self.rng_queue.remove(0);
        }
        let state = self.rng_streams.entry(key.to_owned()).or_insert_with(|| {
            key.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
                (h ^ u64::from(b)).wrapping_mul(0x1_0000_0000_01b3)
            })
        });
        *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = *state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn model(&mut self, call: &HostCall) -> Option<HostRet> {
        Some(match call {
            HostCall::Core(CoreCall::Log { msg }) => {
                self.logs.push(msg.clone());
                HostRet::Unit
            }
            HostCall::Core(CoreCall::RuntimeSide) => HostRet::RuntimeSide(RuntimeSide::Server),
            HostCall::Core(CoreCall::CurrentTick) => HostRet::U64(self.tick),
            HostCall::Core(CoreCall::RngU64 { stream_key }) => {
                HostRet::U64(self.rng_next(stream_key))
            }
            HostCall::Core(CoreCall::RegisterTickSystem { .. })
            | HostCall::Core(CoreCall::RegisterEventHandler { .. })
            | HostCall::Core(CoreCall::RegisterHostileSpawner { .. })
            | HostCall::Core(CoreCall::RegisterBlockBehavior { .. })
            | HostCall::Core(CoreCall::RegisterAiNode { .. })
            | HostCall::Core(CoreCall::EmitEvent { .. }) => HostRet::Unit,
            HostCall::Block(BlockCall::GetBlock { pos }) => HostRet::Block(self.read_block(pos)),
            HostCall::Block(BlockCall::GetBlocks { positions }) => {
                HostRet::Blocks(positions.iter().map(|p| self.read_block(p)).collect())
            }
            HostCall::Block(BlockCall::IsLoaded { pos }) => HostRet::Bool(self.loaded(pos)),
            HostCall::Block(BlockCall::SetBlock { pos, block })
            | HostCall::Block(BlockCall::SwapBlock { pos, block }) => {
                HostRet::Bool(self.write_block(*pos, *block))
            }
            HostCall::Block(BlockCall::SetBlocks { blocks }) => HostRet::U64(
                blocks
                    .iter()
                    .filter(|(pos, block)| self.write_block(*pos, *block))
                    .count() as u64,
            ),
            HostCall::Kv(KvCall::WorldKvGet { key }) => {
                HostRet::Bytes(self.world_kv.get(key).cloned())
            }
            HostCall::Kv(KvCall::WorldKvSet { key, value }) => {
                self.world_kv.insert(key.clone(), value.clone());
                HostRet::Unit
            }
            HostCall::Kv(KvCall::WorldKvDelete { key }) => {
                HostRet::Bool(self.world_kv.remove(key).is_some())
            }
            HostCall::Kv(KvCall::SectionKvGet { pos, key }) => {
                HostRet::Bytes(self.cell_kv.get(&(*pos, key.clone())).cloned())
            }
            HostCall::Kv(KvCall::SectionKvSet { pos, key, value }) => {
                let loaded = self.loaded(pos);
                if loaded {
                    self.cell_kv.insert((*pos, key.clone()), value.clone());
                }
                HostRet::Bool(loaded)
            }
            HostCall::Kv(KvCall::SectionKvDelete { pos, key }) => {
                HostRet::Bool(self.cell_kv.remove(&(*pos, key.clone())).is_some())
            }
            HostCall::Kv(KvCall::SectionKvGetMany { key, positions }) => HostRet::BytesMany(
                positions
                    .iter()
                    .map(|pos| self.cell_kv.get(&(*pos, key.clone())).cloned())
                    .collect(),
            ),
            HostCall::Kv(KvCall::SectionKvSetMany { key, writes }) => HostRet::Bools(
                writes
                    .iter()
                    .map(|(pos, value)| match value {
                        _ if !self.loaded(pos) => false,
                        Some(value) => {
                            self.cell_kv.insert((*pos, key.clone()), value.clone());
                            true
                        }
                        None => self.cell_kv.remove(&(*pos, key.clone())).is_some(),
                    })
                    .collect(),
            ),
            HostCall::Tag(TagCall::MobTagGet { mob_id, key }) => {
                HostRet::MobTag(match self.mobs.get(mob_id) {
                    None => MobTagLookup::MissingMob,
                    Some(tags) => tags
                        .get(key)
                        .cloned()
                        .map_or(MobTagLookup::Absent, MobTagLookup::Value),
                })
            }
            HostCall::Tag(TagCall::MobTagSet { mob_id, key, value }) => {
                HostRet::Bool(self.mobs.get_mut(mob_id).is_some_and(|tags| {
                    tags.insert(key.clone(), value.clone());
                    true
                }))
            }
            HostCall::Tag(TagCall::MobTagDelete { mob_id, key }) => HostRet::Bool(
                self.mobs
                    .get_mut(mob_id)
                    .is_some_and(|tags| tags.remove(key).is_some()),
            ),
            HostCall::Container(ContainerCall::ContainerGet { at }) => {
                HostRet::ContainerSlots(self.containers.get(at).cloned())
            }
            HostCall::Container(ContainerCall::ContainerGetMany { addresses }) => {
                HostRet::Containers(
                    addresses
                        .iter()
                        .map(|at| self.containers.get(at).cloned())
                        .collect(),
                )
            }
            HostCall::Container(ContainerCall::ContainerSet { at, slots }) => {
                let Some(container) = self.containers.get_mut(at) else {
                    return Some(HostRet::Bool(false));
                };
                if slots
                    .iter()
                    .any(|(index, _)| *index as usize >= container.len())
                {
                    return Some(HostRet::Bool(false));
                }
                for (index, stack) in slots {
                    container[*index as usize] = stack.clone();
                }
                HostRet::Bool(true)
            }
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk_calls_reach_the_mock_installed_on_this_thread() {
        let host = MockHost::new();
        host.set_block([1, 2, 3], BlockId(7));
        host.run(|| {
            assert_eq!(crate::get_block([1, 2, 3]), Some(BlockId(7)));
            crate::log("hello");
        });
        assert_eq!(host.logs(), vec!["hello".to_owned()]);
        assert!(matches!(
            host.calls()[0],
            HostCall::Block(BlockCall::GetBlock { pos: [1, 2, 3] })
        ));
    }

    #[test]
    fn hosts_are_per_test_and_nest() {
        let outer = MockHost::new();
        let inner = MockHost::new();
        outer.set_tick(5);
        inner.set_tick(9);
        let _outer = outer.install();
        assert_eq!(crate::current_tick(), 5);
        inner.run(|| assert_eq!(crate::current_tick(), 9));
        assert_eq!(crate::current_tick(), 5, "the outer host is restored");
    }

    #[test]
    fn unmodelled_calls_can_be_answered_by_the_test() {
        let host = MockHost::new();
        host.on(|call| match call {
            HostCall::Core(CoreCall::CurrentTick) => Some(HostRet::U64(42)),
            _ => None,
        });
        host.run(|| assert_eq!(crate::current_tick(), 42));
    }

    #[test]
    fn kv_and_tags_round_trip_and_rng_is_deterministic() {
        let host = MockHost::new();
        host.add_mob(3);
        host.queue_rng([11]);
        let (first, second) = host.run(|| {
            crate::__rt::host_call(&HostCall::Kv(KvCall::WorldKvSet {
                key: "m:k".into(),
                value: vec![1],
            }));
            crate::__rt::host_call(&HostCall::Tag(TagCall::MobTagSet {
                mob_id: 3,
                key: "m:t".into(),
                value: MobTagValue::Bool(true),
            }));
            (crate::rng_u64("s"), crate::rng_u64("s"))
        });
        assert_eq!(host.world_kv("m:k"), Some(vec![1]));
        assert_eq!(host.mob_tag(3, "m:t"), Some(MobTagValue::Bool(true)));
        assert_eq!(first, 11, "queued values come first");
        let replay = MockHost::new().run(|| crate::rng_u64("s"));
        assert_eq!(second, replay, "streams are deterministic per key");
    }

    #[test]
    fn container_reads_and_writes_share_the_fake_world() {
        let host = MockHost::new();
        let at = ContainerAddress::Block([4, 5, 6]);
        let stack = ItemStackData {
            item: "example:stone".into(),
            count: 3,
            data: Vec::new(),
        };
        host.set_container(at, vec![None, None]);
        host.run(|| {
            assert!(crate::container_set(at, vec![(1, Some(stack.clone()))]));
            assert_eq!(
                crate::container_get(at),
                Some(vec![None, Some(stack.clone())])
            );
            assert_eq!(
                crate::container_get_many(vec![at, ContainerAddress::Mob(99)]),
                vec![Some(vec![None, Some(stack)]), None]
            );
            assert!(!crate::container_set(at, vec![(2, None)]));
        });
    }

    #[test]
    #[should_panic(expected = "MockHost does not model")]
    fn unmodelled_calls_panic_with_the_call_named() {
        MockHost::new()
            .run(|| crate::__rt::host_call(&HostCall::Player(mod_api::PlayerCall::Players)));
    }
}
