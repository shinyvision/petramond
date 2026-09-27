//! Shared machine plumbing for container-slot mod machines: the generic
//! placed-machine driver ([`Machine`] over a [`MachineSpec`]), the persisted
//! anchor registry every machine kind keeps in world KV, and the
//! session-stable registry caches (item info, per-class recipe results)
//! machine steps read every tick.
//!
//! A machine KIND is a [`MachineSpec`] impl — its block/variant/kind/anchor/
//! state keys plus one `step`. Everything else (init, placement tracking, the
//! viewer set, and the whole batched tick preamble) is this crate's and is
//! never re-implemented per pack. The kitchen's oven and miller and the
//! forge's forging furnace are all specs over the same driver, and every
//! fuelled one keeps its fire in a [`Burner`].
//!
//! # Cost shape (the thing this crate exists to own)
//!
//! A tick costs a FIXED number of host crossings per machine kind, not one per
//! placed machine: the anchors are probed by section, then read, stepped, and
//! written back through the batched calls (`get_blocks`,
//! `container_get_many`, `section_kv_get_many` / `_set_many`,
//! `set_model_parts_many`, `set_block_draws`). Every one of those is PAGED at
//! the host's batch cap, so a world with more machines than the cap keeps
//! working instead of erroring the call — which disables the mod.
//!
//! Sections are the unit because loading is: every anchor in a section reads
//! the same way, so ONE probe per section decides whether its machines are
//! worth reading at all. A world with ten thousand placed machines and two
//! loaded rooms pays for the two rooms.

use std::collections::{HashMap, HashSet};

use mod_sdk::*;

mod burner;
pub use burner::Burner;

const ANCHOR_BYTES: usize = 12;

const ANCHORS_PER_SHARD: usize = 4096;

const _: () = assert!(ANCHORS_PER_SHARD * ANCHOR_BYTES <= KV_MAX_VALUE_BYTES);

pub trait MachineSpec: Default {
    const KIND_KEY: &'static str;
    const BLOCK_KEY: &'static str;
    const VARIANT_KEYS: &'static [&'static str];
    const ANCHORS_KEY: &'static str;
    const STATE_KEY: &'static str;
    const PANEL_KEYS: &'static [&'static str] = &[];
    const AUX_KEYS: &'static [&'static str] = &[];

    fn init(&mut self) {}

    /// One machine's game tick. `slots` is `None` while no container exists
    /// at the anchor (never opened, never written); `state` is this cell's
    /// [`STATE_KEY`](Self::STATE_KEY) blob, written back by the driver only if
    /// the step changed it (empty = nothing stored).
    ///
    /// Presentation goes into `out` rather than out through a host call, so
    /// the whole kind submits in one crossing. Container writes and block
    /// flips still cross per machine — they are rare and change-gated.
    fn step(
        &mut self,
        ctx: &StepCtx<'_>,
        caches: &mut Caches,
        slots: Option<Vec<Option<ItemStackData>>>,
        state: &mut Vec<u8>,
        out: &mut Presentation,
    );

    /// This anchor is no longer one of ours: the machine was broken, or the
    /// cell was replaced. Drop whatever the kind persists there.
    ///
    /// THE ENGINE DOES NOT CLEAR A MOD'S CELL KV WHEN A BLOCK IS BROKEN, and
    /// it should not have to — the key is the mod's vocabulary. But a machine
    /// that only ever WRITES its blob leaves it behind, and the next machine
    /// placed on that cell decodes it and comes up holding metal it never
    /// melted, or mid-pour. That is not a leak, it is a machine that lies.
    ///
    /// The signal is the anchor PRUNE rather than a `BlockBroken` handler on
    /// purpose: by the time that event fires the footprint is already air, so
    /// a mod cannot resolve the multi-cell anchor from the cell that was hit —
    /// and the registry here already knows every anchor by construction.
    /// It is also self-healing: a machine broken while its section was
    /// unloaded is pruned (and forgotten) the next time it reads.
    fn forget(&mut self, _pos: [i32; 3]) {}
}

pub struct StepCtx<'a> {
    pub pos: [i32; 3],
    pub current: BlockId,
    pub block: BlockId,
    variants: &'a [Option<BlockId>],
    pub aux: &'a [Vec<u8>],
    pub viewers: &'a [PlayerId],
}

impl StepCtx<'_> {
    pub fn gui_open(&self) -> bool {
        !self.viewers.is_empty()
    }

    pub fn publish(&self, key: &str, value: GuiValue) {
        for &player in self.viewers {
            gui_state_set_for(player, key, value.clone());
        }
    }

    pub fn variant(&self, i: usize) -> Option<BlockId> {
        self.variants.get(i).copied().flatten()
    }

    pub fn variant_or_base(&self, i: usize) -> BlockId {
        self.variant(i).unwrap_or(self.block)
    }
}

pub struct Machine<S: MachineSpec> {
    block: Option<BlockId>,
    variants: Vec<Option<BlockId>>,
    anchors: AnchorRegistry,
    spec: S,
}

impl<S: MachineSpec> Default for Machine<S> {
    fn default() -> Self {
        Machine {
            block: None,
            variants: Vec::new(),
            anchors: AnchorRegistry::new(S::ANCHORS_KEY),
            spec: S::default(),
        }
    }
}

impl<S: MachineSpec> Machine<S> {
    pub fn init(&mut self) -> bool {
        self.block = resolve_block_logged(S::BLOCK_KEY);
        if self.block.is_none() {
            return false;
        }
        self.variants = S::VARIANT_KEYS
            .iter()
            .map(|key| resolve_block_logged(key))
            .collect();
        self.anchors.load();
        self.spec.init();
        true
    }

    pub fn on_placed(&mut self, pos: [i32; 3], block: BlockId) {
        if Some(block) == self.block || self.variants.contains(&Some(block)) {
            self.anchors.record(pos);
        }
    }

    pub fn is_present(&self, pos: [i32; 3]) -> bool {
        get_block(pos).is_some_and(|b| Some(b) == self.block || self.variants.contains(&Some(b)))
    }

    pub fn spec(&self) -> &S {
        &self.spec
    }

    pub fn spec_mut(&mut self) -> &mut S {
        &mut self.spec
    }

    pub fn on_container_opened(&mut self, kind: &ContainerKind, at: Option<ContainerAddress>) {
        if !kind.is(S::KIND_KEY) && !S::PANEL_KEYS.iter().any(|k| kind.is(k)) {
            return;
        }
        if let Some(ContainerAddress::Block(anchor)) = at {
            self.anchors.record(anchor);
        }
    }

    pub fn tick(&mut self, caches: &mut Caches) {
        let Machine {
            block,
            variants,
            anchors,
            spec,
        } = self;
        let Some(block) = *block else {
            return;
        };
        if anchors.is_empty() {
            return;
        }
        let Pruned { live, gone } =
            anchors.prune_live(|b| b == block || variants.contains(&Some(b)));
        for pos in gone {
            spec.forget(pos);
        }
        if live.is_empty() {
            return;
        }
        let positions: Vec<[i32; 3]> = live.iter().map(|(p, _)| *p).collect();
        let containers = paged(positions.clone(), |page| {
            container_get_many(page.into_iter().map(ContainerAddress::from).collect())
        });
        let states = paged(positions.clone(), |page| {
            section_kv_get_many(S::STATE_KEY, page)
        });
        let auxiliary: Vec<_> = S::AUX_KEYS
            .iter()
            .map(|key| paged(positions.clone(), |page| section_kv_get_many(key, page)))
            .collect();
        let watchers = Watchers::of_kinds(S::KIND_KEY, S::PANEL_KEYS);
        let mut out = Presentation::default();
        for (i, (((pos, current), slots), state)) in
            live.into_iter().zip(containers).zip(states).enumerate()
        {
            let aux: Vec<_> = auxiliary
                .iter()
                .map(|values| values[i].clone().unwrap_or_default())
                .collect();
            let mut state = state.unwrap_or_default();
            let before_state = state.clone();
            let ctx = StepCtx {
                pos,
                current,
                block,
                variants,
                viewers: watchers.at(pos),
                aux: &aux,
            };
            spec.step(&ctx, caches, slots, &mut state, &mut out);
            if state != before_state {
                out.state.push((pos, (!state.is_empty()).then_some(state)));
            }
        }
        out.flush(S::STATE_KEY);
    }
}

#[derive(Default)]
pub struct Presentation {
    draws: Vec<([i32; 3], Vec<DrawPrim>)>,
    parts: Vec<([i32; 3], u32, Option<[u8; 3]>)>,
    state: Vec<([i32; 3], Option<Vec<u8>>)>,
}

impl Presentation {
    pub fn parts(&mut self, pos: [i32; 3], mask: u32, tint: Option<[u8; 3]>) {
        self.parts.push((pos, mask, tint));
    }

    pub fn draw(&mut self, pos: [i32; 3], prims: Vec<DrawPrim>) {
        self.draws.push((pos, prims));
    }

    fn flush(self, state_key: &str) {
        let Presentation {
            draws,
            parts,
            state,
        } = self;
        paged(state, |page| section_kv_set_many(state_key, page));
        paged(parts, set_model_parts_many);
        paged(draws, set_block_draws);
    }
}

struct Watchers {
    by_anchor: HashMap<[i32; 3], Vec<PlayerId>>,
}

impl Watchers {
    fn of_kinds(kind_key: &str, panels: &[&str]) -> Watchers {
        let mut by_anchor: HashMap<[i32; 3], Vec<PlayerId>> = HashMap::new();
        for viewer in gui_viewers() {
            let (true, Some(ContainerAddress::Block(anchor))) = (
                viewer.kind == kind_key || panels.contains(&viewer.kind.as_str()),
                viewer.anchor,
            ) else {
                continue;
            };
            by_anchor.entry(anchor).or_default().push(viewer.player_id);
        }
        Watchers { by_anchor }
    }

    fn at(&self, anchor: [i32; 3]) -> &[PlayerId] {
        self.by_anchor.get(&anchor).map_or(&[], Vec::as_slice)
    }
}

pub struct Pruned {
    pub live: Vec<([i32; 3], BlockId)>,
    pub gone: Vec<[i32; 3]>,
}

/// The placed-anchor list for one machine kind, persisted in world KV as
/// [`ANCHOR_BYTES`]-wide LE cell records in placement order (the deterministic tick order),
/// SHARDED so the list is not bounded by one KV value's cap.
///
/// Two things here are scalability, not bookkeeping:
///
/// - **Shards.** Shard 0 keeps the plain key (old worlds load unchanged);
///   shard `n` is `<key>/<n>`. A placement rewrites only the LAST shard, so
///   building the thousandth machine costs the same write as the first.
/// - **Section probing.** Every anchor in a 16³ section reads the same way
///   (loaded or not), so [`prune_live`](Self::prune_live) asks ONE cell per
///   section first and skips whole sections whose machines are unloaded. A
///   world with ten thousand placed machines and two loaded rooms then costs
///   one read per section plus the two rooms, instead of ten thousand.
pub struct AnchorRegistry {
    kv_key: &'static str,
    anchors: Vec<[i32; 3]>,
    seen: HashSet<[i32; 3]>,
}

fn section_of(pos: [i32; 3]) -> [i32; 3] {
    [pos[0] >> 4, pos[1] >> 4, pos[2] >> 4]
}

impl AnchorRegistry {
    pub fn new(kv_key: &'static str) -> Self {
        AnchorRegistry {
            kv_key,
            anchors: Vec::new(),
            seen: HashSet::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.anchors.is_empty()
    }

    pub fn anchors(&self) -> &[[i32; 3]] {
        &self.anchors
    }

    fn shard_key(&self, i: usize) -> String {
        if i == 0 {
            self.kv_key.to_owned()
        } else {
            format!("{}/{i}", self.kv_key)
        }
    }

    pub fn load(&mut self) {
        self.anchors.clear();
        self.seen.clear();
        for i in 0.. {
            let Some(bytes) = world_kv_get(&self.shard_key(i)) else {
                break;
            };
            let mut r = ByteReader::new(&bytes);
            while let Some(pos) = r.i32x3() {
                if self.seen.insert(pos) {
                    self.anchors.push(pos);
                }
            }
        }
    }

    fn encode_shard(&self, i: usize) -> Vec<u8> {
        let from = i * ANCHORS_PER_SHARD;
        let to = (from + ANCHORS_PER_SHARD).min(self.anchors.len());
        let mut w = ByteWriter::with_capacity(to.saturating_sub(from) * ANCHOR_BYTES);
        for pos in &self.anchors[from..to.max(from)] {
            w.i32x3(*pos);
        }
        w.finish()
    }

    fn shard_count(&self) -> usize {
        self.anchors.len().div_ceil(ANCHORS_PER_SHARD).max(1)
    }

    fn store(&self) {
        let shards = self.shard_count();
        for i in 0..shards {
            world_kv_set(&self.shard_key(i), self.encode_shard(i));
        }
        let mut i = shards;
        while world_kv_delete(&self.shard_key(i)) {
            i += 1;
        }
    }

    pub fn record(&mut self, pos: [i32; 3]) {
        if !self.seen.insert(pos) {
            return;
        }
        self.anchors.push(pos);
        let last = self.shard_count() - 1;
        world_kv_set(&self.shard_key(last), self.encode_shard(last));
    }

    /// Prune stale anchors and return the live `(anchor, current block)` pairs
    /// plus the anchors just DROPPED.
    ///
    /// `None` reads (unloaded / streaming) are neither pruned nor live — their
    /// state is frozen on disk; only a real foreign-block read prunes (the
    /// host guarantees half-streamed sections read `None`, never their
    /// pre-overlay base). That is also what makes the section probe EXACT:
    /// a section reading `None` at one cell reads `None` at all of them, so
    /// skipping its anchors wholesale reaches the same answer as reading them.
    ///
    /// The dropped list is returned rather than swallowed because a prune is
    /// the only moment a machine kind learns one of its machines is GONE, and
    /// per-cell state it wrote there outlives the block otherwise.
    pub fn prune_live(&mut self, is_ours: impl Fn(BlockId) -> bool) -> Pruned {
        let mut probes: Vec<[i32; 3]> = Vec::new();
        let mut probe_of: HashMap<[i32; 3], usize> = HashMap::new();
        for pos in &self.anchors {
            let sp = section_of(*pos);
            if let std::collections::hash_map::Entry::Vacant(e) = probe_of.entry(sp) {
                e.insert(probes.len());
                probes.push(*pos);
            }
        }
        let loaded: Vec<bool> = paged(probes, get_blocks)
            .into_iter()
            .map(|b| b.is_some())
            .collect();

        let candidates: Vec<[i32; 3]> = self
            .anchors
            .iter()
            .copied()
            .filter(|pos| probe_of.get(&section_of(*pos)).is_some_and(|&i| loaded[i]))
            .collect();
        let blocks = paged(candidates.clone(), get_blocks);

        let mut live = Vec::new();
        let mut gone = Vec::new();
        for (pos, block) in candidates.into_iter().zip(blocks) {
            match block {
                None => continue,
                Some(b) if is_ours(b) => live.push((pos, b)),
                Some(_) => gone.push(pos),
            }
        }
        if !gone.is_empty() {
            let dropped: HashSet<[i32; 3]> = gone.iter().copied().collect();
            self.anchors.retain(|p| !dropped.contains(p));
            self.seen.retain(|p| !dropped.contains(p));
            self.store();
        }
        Pruned { live, gone }
    }
}

#[derive(Default)]
pub struct Caches {
    fuel_ticks: HashMap<String, u32>,
    max_stack: HashMap<String, u8>,
    recipes: HashMap<(String, String), Option<ItemStackData>>,
}

impl Caches {
    pub fn fuel_ticks_for(&mut self, item: &str) -> u32 {
        if let Some(&t) = self.fuel_ticks.get(item) {
            return t;
        }
        let t = item_info(item).map(|i| i.fuel_burn_ticks).unwrap_or(0);
        self.fuel_ticks.insert(item.to_owned(), t);
        t
    }

    pub fn max_stack_for(&mut self, item: &str) -> u8 {
        if let Some(&m) = self.max_stack.get(item) {
            return m;
        }
        let m = item_info(item).map(|i| i.max_stack).unwrap_or(64);
        self.max_stack.insert(item.to_owned(), m);
        m
    }

    pub fn recipe_for(&mut self, class: &str, item: &str) -> Option<ItemStackData> {
        let key = (class.to_owned(), item.to_owned());
        if let Some(cached) = self.recipes.get(&key) {
            return cached.clone();
        }
        let result = recipe_result(class, item);
        self.recipes.insert(key, result.clone());
        result
    }
}

pub fn output_accepts(
    caches: &mut Caches,
    output: &Option<ItemStackData>,
    result: &ItemStackData,
) -> bool {
    match output {
        None => true,
        Some(o) => {
            o.item == result.item
                && o.data == result.data
                && caches.max_stack_for(&o.item).saturating_sub(o.count) >= result.count
        }
    }
}

pub fn merge_output(output: &mut Option<ItemStackData>, result: &ItemStackData) {
    *output = Some(match output.take() {
        None => result.clone(),
        Some(o) => ItemStackData {
            count: o.count + result.count,
            ..o
        },
    });
}

pub fn consume_one(slot: &mut Option<ItemStackData>) {
    if let Some(s) = slot.take() {
        *slot = (s.count > 1).then(|| ItemStackData {
            count: s.count - 1,
            ..s
        });
    }
}

pub fn write_changed_slots(
    pos: [i32; 3],
    before: &[Option<ItemStackData>],
    after: &[Option<ItemStackData>],
) {
    let writes = changed_slots(before, after);
    if !writes.is_empty() {
        container_set(pos.into(), writes);
    }
}

pub fn changed_slots(
    before: &[Option<ItemStackData>],
    after: &[Option<ItemStackData>],
) -> Vec<(u32, Option<ItemStackData>)> {
    (0..before.len().max(after.len()))
        .filter(|&i| slot_at(before, i) != slot_at(after, i))
        .map(|i| (i as u32, slot_at(after, i).clone()))
        .collect()
}

fn slot_at(slots: &[Option<ItemStackData>], i: usize) -> &Option<ItemStackData> {
    const EMPTY: &Option<ItemStackData> = &None;
    slots.get(i).unwrap_or(EMPTY)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack(item: &str, count: u8) -> ItemStackData {
        ItemStackData {
            item: item.into(),
            count,
            data: Vec::new(),
        }
    }

    #[test]
    fn the_last_of_an_input_leaves_an_empty_slot() {
        let mut slot = Some(stack("petramond:coal", 1));
        consume_one(&mut slot);
        assert!(slot.is_none());

        let mut slot = Some(stack("petramond:coal", 3));
        consume_one(&mut slot);
        assert_eq!(slot.map(|s| s.count), Some(2));
    }

    #[test]
    fn merging_output_keeps_the_stored_stack() {
        let mut out = Some(ItemStackData {
            data: vec![("forge:heat".into(), "1".into())],
            ..stack("forge:iron_ingot", 2)
        });
        merge_output(&mut out, &stack("forge:iron_ingot", 3));
        let out = out.unwrap();
        assert_eq!(out.count, 5);
        assert_eq!(out.data.len(), 1, "the stored stack's data survives");
    }

    #[test]
    fn the_anchor_list_shards_and_shard_zero_keeps_the_plain_key() {
        let mut reg = AnchorRegistry::new("m:anchors");
        assert_eq!(reg.shard_key(0), "m:anchors");
        assert_eq!(reg.shard_key(2), "m:anchors/2");
        assert_eq!(reg.shard_count(), 1, "an empty list is still one record");

        reg.anchors = (0..ANCHORS_PER_SHARD as i32 + 3)
            .map(|i| [i, 0, 0])
            .collect();
        assert_eq!(reg.shard_count(), 2);
        assert_eq!(reg.encode_shard(0).len(), ANCHORS_PER_SHARD * ANCHOR_BYTES);
        assert_eq!(reg.encode_shard(1).len(), 3 * ANCHOR_BYTES);
        assert!(reg.encode_shard(0).len() <= KV_MAX_VALUE_BYTES);

        let mut bytes = reg.encode_shard(0);
        bytes.extend(reg.encode_shard(1));
        let mut r = ByteReader::new(&bytes);
        let mut back = Vec::new();
        while let Some(p) = r.i32x3() {
            back.push(p);
        }
        assert_eq!(back, reg.anchors);
    }

    #[test]
    fn slot_writeback_diffs_and_tolerates_a_length_change() {
        let before = vec![Some(stack("a", 1)), None, Some(stack("c", 2))];
        let after = vec![Some(stack("a", 1)), Some(stack("b", 1))];
        let changed = changed_slots(&before, &after);
        assert_eq!(
            changed.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
            vec![1, 2],
            "the moved slot AND the one `after` dropped off its end"
        );
        assert_eq!(changed[0].1.as_ref().map(|s| s.item.as_str()), Some("b"));
        assert!(changed[1].1.is_none(), "the dropped tail is CLEARED");

        assert_eq!(changed_slots(&before, &before[..1]), vec![(2, None)]);
        let grown = vec![
            Some(stack("a", 1)),
            None,
            Some(stack("c", 2)),
            Some(stack("d", 1)),
        ];
        assert_eq!(
            changed_slots(&before, &grown)
                .iter()
                .map(|(i, _)| *i)
                .collect::<Vec<_>>(),
            vec![3]
        );
    }
}
