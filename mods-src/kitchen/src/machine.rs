//! Shared machine plumbing: the generic placed-machine driver ([`Machine`]
//! over a [`MachineSpec`]), the persisted anchor registry every machine kind
//! keeps in world KV, and the session-stable registry caches (item info,
//! per-class recipe results) machine steps read every tick.

use std::collections::HashMap;

use mod_sdk::*;

/// One machine KIND's identity plus its per-tick step. Everything a placed
/// machine shares — the persisted anchor registry, container-session
/// tracking, and the batched prune/read tick preamble — is [`Machine`];
/// a spec only says which rows are its and what one machine does per tick.
pub trait MachineSpec: Default {
    /// The mod GUI container kind (`ContainerKind::Mod` key).
    const KIND_KEY: &'static str;
    /// The placeable base block row; unresolvable = this kind stays idle.
    const BLOCK_KEY: &'static str;
    /// The same-footprint visual-variant row (lit oven, full miller);
    /// unresolvable degrades the visual flip, never the machine.
    const VARIANT_KEY: &'static str;
    /// World-KV key of the persisted anchor list.
    const ANCHORS_KEY: &'static str;

    /// One machine's game tick. `slots` is `None` while no container exists
    /// at the anchor (never opened, never written). Write back only what
    /// changed — container writes, cell KV, and block flips all cross the ABI.
    fn step(
        &mut self,
        ctx: &StepCtx,
        caches: &mut Caches,
        slots: Option<Vec<Option<ItemStackData>>>,
    );
}

/// One live machine's tick context: where it is, what its anchor currently
/// decodes to, the resolved rows, and whether its GUI session is open (the
/// gauge publish gate).
pub struct StepCtx {
    pub pos: [i32; 3],
    /// The anchor's current block this tick (base or variant).
    pub current: BlockId,
    pub block: BlockId,
    pub variant: Option<BlockId>,
    pub gui_open: bool,
}

/// The shared driver for one placed-machine kind: resolved rows, the anchor
/// registry, and the open GUI session, stepped through the spec each tick.
pub struct Machine<S: MachineSpec> {
    block: Option<BlockId>,
    /// `None` degrades to working with no visual flip, never to not working.
    variant: Option<BlockId>,
    anchors: AnchorRegistry,
    /// The machine whose GUI session is open, if any (gauge publish gate).
    open_session: Option<[i32; 3]>,
    spec: S,
}

impl<S: MachineSpec> Default for Machine<S> {
    fn default() -> Self {
        Machine {
            block: None,
            variant: None,
            anchors: AnchorRegistry::new(S::ANCHORS_KEY),
            open_session: None,
            spec: S::default(),
        }
    }
}

impl<S: MachineSpec> Machine<S> {
    /// Resolve blocks + restore the anchor list; `false` = the pack's rows
    /// are missing and this machine kind stays idle.
    pub fn init(&mut self) -> bool {
        self.block = resolve_block_logged(S::BLOCK_KEY);
        if self.block.is_none() {
            return false;
        }
        self.variant = resolve_block_logged(S::VARIANT_KEY);
        self.anchors.load();
        true
    }

    /// `block_placed.pos` is the multi-cell anchor — the same cell the engine
    /// keys the container at.
    pub fn on_placed(&mut self, pos: [i32; 3], block: BlockId) {
        if Some(block) == self.block {
            self.anchors.record(pos);
        }
    }

    /// Container session tracking; opening also self-heals a lost anchor.
    pub fn on_container(&mut self, kind: &ContainerKind, pos: Option<[i32; 3]>, opened: bool) {
        if !matches!(kind, ContainerKind::Mod { key } if key == S::KIND_KEY) {
            return;
        }
        if opened {
            self.open_session = pos;
            if let Some(anchor) = pos {
                self.anchors.record(anchor);
            }
        } else {
            self.open_session = None;
        }
    }

    /// Prune stale anchors, read every live machine's container in ONE
    /// batched call (never `container_get` in a loop), and step each one.
    pub fn tick(&mut self, caches: &mut Caches) {
        let Some(block) = self.block else {
            return;
        };
        if self.anchors.anchors.is_empty() {
            return;
        }
        let variant = self.variant;
        let live = self.anchors.prune_live(|b| b == block || Some(b) == variant);
        if live.is_empty() {
            return;
        }
        let positions: Vec<[i32; 3]> = live.iter().map(|(p, _)| *p).collect();
        let containers = container_get_many(positions);
        for ((pos, current), slots) in live.into_iter().zip(containers) {
            let ctx = StepCtx {
                pos,
                current,
                block,
                variant,
                gui_open: self.open_session == Some(pos),
            };
            self.spec.step(&ctx, caches, slots);
        }
    }
}

/// The placed-anchor list for one machine kind, persisted in world KV as
/// 12-byte LE cell records, in placement order (the deterministic tick
/// order). Self-healingly pruned by [`prune_live`](Self::prune_live) when a
/// listed cell no longer decodes to one of the machine's block variants.
pub struct AnchorRegistry {
    kv_key: &'static str,
    pub anchors: Vec<[i32; 3]>,
}

impl AnchorRegistry {
    pub fn new(kv_key: &'static str) -> Self {
        AnchorRegistry {
            kv_key,
            anchors: Vec::new(),
        }
    }

    pub fn load(&mut self) {
        self.anchors.clear();
        if let Some(bytes) = world_kv_get(self.kv_key) {
            let mut r = ByteReader::new(&bytes);
            while let Some(pos) = r.i32x3() {
                self.anchors.push(pos);
            }
        }
    }

    fn store(&self) {
        let mut w = ByteWriter::with_capacity(self.anchors.len() * 12);
        for pos in &self.anchors {
            w.i32x3(*pos);
        }
        world_kv_set(self.kv_key, w.finish());
    }

    /// Record a newly placed (or GUI-self-healed) anchor.
    pub fn record(&mut self, pos: [i32; 3]) {
        if !self.anchors.contains(&pos) {
            self.anchors.push(pos);
            self.store();
        }
    }

    /// One batched block read that prunes stale anchors and returns the live
    /// `(anchor, current block)` pairs. `None` reads (unloaded / streaming)
    /// are neither pruned nor live — their state is frozen on disk; only a
    /// real foreign-block read prunes (the host guarantees half-streamed
    /// sections read `None`, never their pre-overlay base).
    pub fn prune_live(&mut self, is_ours: impl Fn(BlockId) -> bool) -> Vec<([i32; 3], BlockId)> {
        let positions = self.anchors.clone();
        let blocks = get_blocks(positions.clone());
        let mut live = Vec::new();
        let mut pruned = false;
        for (pos, block) in positions.into_iter().zip(blocks) {
            match block {
                None => continue,
                Some(b) if is_ours(b) => live.push((pos, b)),
                Some(_) => {
                    self.anchors.retain(|p| *p != pos);
                    pruned = true;
                }
            }
        }
        if pruned {
            self.store();
        }
        live
    }
}

/// Session caches for registry data (stable per session — never re-ask the
/// host per tick): item stack caps, fuel burn ticks, and per-(class, input)
/// machine recipe results.
#[derive(Default)]
pub struct Caches {
    fuel_ticks: HashMap<String, u32>,
    max_stack: HashMap<String, u8>,
    recipes: HashMap<(&'static str, String), Option<ItemStackData>>,
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

    /// The loaded `class` recipe result for input `item` (registry name), or
    /// `None` for no recipe.
    pub fn recipe_for(&mut self, class: &'static str, item: &str) -> Option<ItemStackData> {
        if let Some(cached) = self.recipes.get(&(class, item.to_owned())) {
            return cached.clone();
        }
        let result = recipe_result(class, item);
        self.recipes.insert((class, item.to_owned()), result.clone());
        result
    }
}

/// Whether `result` fits into the `output` slot (empty, or same item with
/// stack headroom).
pub fn output_accepts(
    caches: &mut Caches,
    output: &Option<ItemStackData>,
    result: &ItemStackData,
) -> bool {
    match output {
        None => true,
        // Saturating: a stack saved above a row's (since lowered) cap must
        // read as "no headroom", not wrap around.
        Some(o) => {
            o.item == result.item
                && caches.max_stack_for(&o.item).saturating_sub(o.count) >= result.count
        }
    }
}

/// Merge `result` into the `output` slot (the caller checked
/// [`output_accepts`]).
pub fn merge_output(output: &mut Option<ItemStackData>, result: &ItemStackData) {
    *output = Some(match output.take() {
        None => result.clone(),
        Some(o) => ItemStackData {
            item: o.item,
            count: o.count + result.count,
        },
    });
}

/// Decrement a consumed input slot by one.
pub fn consume_one(slot: &mut Option<ItemStackData>) {
    if let Some(s) = slot.take() {
        *slot = (s.count > 1).then(|| ItemStackData {
            item: s.item,
            count: s.count - 1,
        });
    }
}

/// Write back only the slots that changed, as one batched call.
pub fn write_changed_slots(
    pos: [i32; 3],
    before: &[Option<ItemStackData>],
    after: &[Option<ItemStackData>],
) {
    if after != before {
        let writes = (0..after.len())
            .filter(|&i| after[i] != before[i])
            .map(|i| (i as u32, after[i].clone()))
            .collect();
        container_set(pos, writes);
    }
}
