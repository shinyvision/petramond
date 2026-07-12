//! Shared machine plumbing: the persisted anchor registry every placed
//! machine kind keeps in world KV, and the session-stable registry caches
//! (item info, per-class recipe results) machine steps read every tick.

use std::collections::HashMap;

use mod_sdk::*;

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
            for rec in bytes.chunks_exact(12) {
                let word = |i: usize| i32::from_le_bytes(rec[i * 4..i * 4 + 4].try_into().unwrap());
                self.anchors.push([word(0), word(1), word(2)]);
            }
        }
    }

    fn store(&self) {
        let mut bytes = Vec::with_capacity(self.anchors.len() * 12);
        for pos in &self.anchors {
            for c in pos {
                bytes.extend(c.to_le_bytes());
            }
        }
        world_kv_set(self.kv_key, bytes);
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
    pub fn fuel_ticks_for(&mut self, key: &str) -> u32 {
        if let Some(&t) = self.fuel_ticks.get(key) {
            return t;
        }
        let t = item_info(key).map(|i| i.fuel_burn_ticks).unwrap_or(0);
        self.fuel_ticks.insert(key.to_owned(), t);
        t
    }

    pub fn max_stack_for(&mut self, key: &str) -> u8 {
        if let Some(&m) = self.max_stack.get(key) {
            return m;
        }
        let m = item_info(key).map(|i| i.max_stack).unwrap_or(64);
        self.max_stack.insert(key.to_owned(), m);
        m
    }

    /// The loaded `class` recipe result for `key`, or `None` for no recipe.
    pub fn recipe_for(&mut self, class: &'static str, key: &str) -> Option<ItemStackData> {
        if let Some(cached) = self.recipes.get(&(class, key.to_owned())) {
            return cached.clone();
        }
        let result = recipe_result(class, key);
        self.recipes.insert((class, key.to_owned()), result.clone());
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
            o.key == result.key
                && caches.max_stack_for(&o.key).saturating_sub(o.count) >= result.count
        }
    }
}

/// Merge `result` into the `output` slot (the caller checked
/// [`output_accepts`]).
pub fn merge_output(output: &mut Option<ItemStackData>, result: &ItemStackData) {
    *output = Some(match output.take() {
        None => result.clone(),
        Some(o) => ItemStackData {
            key: o.key,
            count: o.count + result.count,
        },
    });
}

/// Decrement a consumed input slot by one.
pub fn consume_one(slot: &mut Option<ItemStackData>) {
    if let Some(s) = slot.take() {
        *slot = (s.count > 1).then(|| ItemStackData {
            key: s.key,
            count: s.count - 1,
        });
    }
}
