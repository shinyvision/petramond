//! Kitchen mod: a craftable multi-cell kitchen oven that cooks like the
//! engine furnace, built entirely on the mod container-slot API.
//!
//! Content is pack data (block/item/recipe/model/GUI document); this crate is
//! only the cooking logic. The engine owns the oven's three `container` slots
//! (declared by the GUI document: 0 = food input, 1 = fuel, 2 = take-only
//! output), stores them per placed oven at the model group's base cell, and
//! routes clicks/shift-clicks; this mod steps burn/cook state each tick and
//! swaps the slots through `container_get`/`container_set`.
//!
//! State model, all deterministic and save-safe:
//! - The oven POSITION LIST lives in world KV `kitchen:ovens` (12 B LE per
//!   anchor), appended on `block_placed` and self-healingly pruned when a
//!   listed cell no longer holds an oven (covers breaks, mod edits, and any
//!   cell of the multi-cell footprint being the reported break position).
//! - Per-oven burn/cook state lives in section cell KV `kitchen:state` at the
//!   anchor (3×u32 LE: cook_progress, burn_remaining, burn_max), so a lit
//!   oven reloads mid-bake exactly like the engine furnace.
//! - While this oven's GUI session is open, the gauges are published to the
//!   GUI state map as `kitchen:cook01` / `kitchen:burn01` (F32).
//!
//! The cook algorithm mirrors the engine furnace's pacing (600-tick cook,
//! regress by 2 while unlit, relight only when there is work), but consumes
//! its OWN recipe class: `kitchen:cooking` rows in any pack's recipes.json
//! (via `recipe_result`), never the furnace's `llama:smelting` table — an
//! oven cooks food, it does not smelt ore. Fuel data comes from `item_info`.
//! Any pack adds `kitchen:cooking` recipes + the `kitchen:cookable` item tag
//! and the oven picks them up with no code change here or in the engine.
//!
//! Shipped food: sheep drop 1–2 `kitchen:raw_mutton` (this pack's
//! `loot_tables.json` layer replaces the sheep table — pure data), which cooks
//! into `kitchen:cooked_mutton`; eating that grants `llama:regeneration`
//! (engine status effect, 1200 ticks) through the item row's `food` data.
//!
//! While burning, the oven flips to the `kitchen:oven_lit` block row (same
//! authored model; the unlit row hides the `fire` cube via `hidden_parts`) —
//! glow and underside fire particles are that row's data. The flip goes
//! through `swap_model_block`, which preserves the container and cell KV.

use mod_sdk::*;
use std::collections::HashMap;

const KIND_KEY: &str = "kitchen:oven";
const OVEN_BLOCK_KEY: &str = "kitchen:oven";
/// The lit variant: the same authored model with the `fire` cube visible,
/// block-light emission, and the underside fire particle emitter — all pack
/// data on its rows. The mod's only job is swapping the placed block between
/// the two on burn transitions (`swap_model_block` keeps container + state).
const LIT_BLOCK_KEY: &str = "kitchen:oven_lit";
const OVEN_LIST_KEY: &str = "kitchen:ovens";
const STATE_KEY: &str = "kitchen:state";

/// The oven's machine-processing recipe class (see pack recipes.json rows).
const COOKING_CLASS: &str = "kitchen:cooking";

const SLOT_INPUT: usize = 0;
const SLOT_FUEL: usize = 1;
const SLOT_OUTPUT: usize = 2;

/// Mirrors the engine furnace's pacing (600-tick cook, 2-tick regress).
const COOK_TICKS: u32 = 600;
const COOK_REGRESS: u32 = 2;

const TICK_SYSTEM: u32 = 1;
const ON_BLOCK_PLACED: u32 = 1;
const ON_CONTAINER_OPENED: u32 = 2;
const ON_CONTAINER_CLOSED: u32 = 3;

#[derive(Clone, Copy, Default, PartialEq)]
struct OvenState {
    cook_progress: u32,
    burn_remaining: u32,
    burn_max: u32,
}

impl OvenState {
    fn decode(bytes: &[u8]) -> OvenState {
        let word = |i: usize| -> u32 {
            bytes
                .get(i * 4..i * 4 + 4)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .unwrap_or(0)
        };
        OvenState {
            cook_progress: word(0),
            burn_remaining: word(1),
            burn_max: word(2),
        }
    }

    fn encode(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(12);
        out.extend(self.cook_progress.to_le_bytes());
        out.extend(self.burn_remaining.to_le_bytes());
        out.extend(self.burn_max.to_le_bytes());
        out
    }
}

#[derive(Default)]
struct Kitchen {
    oven_block: Option<BlockId>,
    /// The lit visual variant (`kitchen:oven_lit`), when the pack ships it.
    /// `None` degrades to cooking with no visual flip, never to not cooking.
    lit_block: Option<BlockId>,
    /// Placed oven anchors, in placement order (the deterministic tick order).
    ovens: Vec<[i32; 3]>,
    /// The oven whose GUI session is open, if any (gauge publish gate).
    open_session: Option<[i32; 3]>,
    /// Session caches for registry data (stable per session).
    fuel_ticks: HashMap<String, u32>,
    cook: HashMap<String, Option<ItemSlotData>>,
    max_stack: HashMap<String, u8>,
}

impl Kitchen {
    fn load_oven_list(&mut self) {
        self.ovens.clear();
        if let Some(bytes) = world_kv_get(OVEN_LIST_KEY) {
            for rec in bytes.chunks_exact(12) {
                let word = |i: usize| i32::from_le_bytes(rec[i * 4..i * 4 + 4].try_into().unwrap());
                self.ovens.push([word(0), word(1), word(2)]);
            }
        }
    }

    fn store_oven_list(&self) {
        let mut bytes = Vec::with_capacity(self.ovens.len() * 12);
        for pos in &self.ovens {
            for c in pos {
                bytes.extend(c.to_le_bytes());
            }
        }
        world_kv_set(OVEN_LIST_KEY, bytes);
    }

    fn fuel_ticks_for(&mut self, key: &str) -> u32 {
        if let Some(&t) = self.fuel_ticks.get(key) {
            return t;
        }
        let t = item_info(key).map(|i| i.fuel_burn_ticks).unwrap_or(0);
        self.fuel_ticks.insert(key.to_owned(), t);
        t
    }

    fn max_stack_for(&mut self, key: &str) -> u8 {
        if let Some(&m) = self.max_stack.get(key) {
            return m;
        }
        let m = item_info(key).map(|i| i.max_stack).unwrap_or(64);
        self.max_stack.insert(key.to_owned(), m);
        m
    }

    fn cook_result_for(&mut self, key: &str) -> Option<ItemSlotData> {
        if let Some(cached) = self.cook.get(key) {
            return cached.clone();
        }
        let result = recipe_result(COOKING_CLASS, key);
        self.cook.insert(key.to_owned(), result.clone());
        result
    }

    /// One oven's game tick: the engine furnace algorithm over the container
    /// slots. Returns nothing; writes back only what changed.
    fn step_oven(&mut self, pos: [i32; 3]) {
        let Some(mut slots) = container_get(pos) else {
            // Never opened and never written: an empty oven has nothing to do.
            return;
        };
        slots.resize(3, None);
        let mut state = OvenState::decode(&section_kv_get(pos, STATE_KEY).unwrap_or_default());
        let before_state = state;
        let before_slots = slots.clone();

        let was_lit = state.burn_remaining > 0;
        if state.burn_remaining > 0 {
            state.burn_remaining -= 1;
        }

        // What the input would cook into (the oven's OWN class), if the
        // output has room for it.
        let result = slots[SLOT_INPUT]
            .as_ref()
            .filter(|s| s.count > 0)
            .map(|s| s.key.clone())
            .and_then(|k| self.cook_result_for(&k));
        let can_cook = result.as_ref().is_some_and(|r| match &slots[SLOT_OUTPUT] {
            None => true,
            Some(o) => o.key == r.key && self.max_stack_for(&o.key) - o.count >= r.count,
        });

        // Relight from the fuel slot only when the flame is out AND there is
        // cookable work — idle fuel is never consumed (the furnace contract).
        if state.burn_remaining == 0 && can_cook {
            if let Some(fuel) = slots[SLOT_FUEL].clone() {
                let burn = self.fuel_ticks_for(&fuel.key);
                if burn > 0 {
                    state.burn_remaining = burn;
                    state.burn_max = burn;
                    slots[SLOT_FUEL] = (fuel.count > 1).then(|| ItemSlotData {
                        key: fuel.key,
                        count: fuel.count - 1,
                    });
                }
            }
        }

        if state.burn_remaining > 0 && can_cook {
            state.cook_progress += 1;
            if state.cook_progress >= COOK_TICKS {
                state.cook_progress = 0;
                let result = result.expect("can_cook implies a result");
                slots[SLOT_OUTPUT] = Some(match slots[SLOT_OUTPUT].take() {
                    None => result.clone(),
                    Some(o) => ItemSlotData {
                        key: o.key,
                        count: o.count + result.count,
                    },
                });
                let input = slots[SLOT_INPUT].take().expect("can_cook implies input");
                slots[SLOT_INPUT] = (input.count > 1).then(|| ItemSlotData {
                    key: input.key,
                    count: input.count - 1,
                });
            }
        } else {
            state.cook_progress = state.cook_progress.saturating_sub(COOK_REGRESS);
        }

        if slots != before_slots {
            let writes = (0..3)
                .filter(|&i| slots[i] != before_slots[i])
                .map(|i| (i as u32, slots[i].clone()))
                .collect();
            container_set(pos, writes);
        }
        if state != before_state {
            section_kv_set(pos, STATE_KEY, state.encode());
        }
        // Flip the placed block between the unlit/lit rows on burn transitions
        // only (the swap is engine-idempotent, but there is no reason to cross
        // the ABI 20×/s per oven). The fire cube, glow, and fire particles are
        // all data on the lit row.
        let now_lit = state.burn_remaining > 0;
        if was_lit != now_lit {
            if let (Some(oven), Some(lit)) = (self.oven_block, self.lit_block) {
                swap_model_block(pos, if now_lit { lit } else { oven });
            }
        }
        if self.open_session == Some(pos) {
            gui_state_set(
                "kitchen:cook01",
                GuiValue::F32(state.cook_progress as f32 / COOK_TICKS as f32),
            );
            let burn01 = if state.burn_max == 0 {
                0.0
            } else {
                state.burn_remaining as f32 / state.burn_max as f32
            };
            gui_state_set("kitchen:burn01", GuiValue::F32(burn01));
        }
    }
}

impl Mod for Kitchen {
    fn init(&mut self) {
        self.oven_block = resolve_block(OVEN_BLOCK_KEY);
        if self.oven_block.is_none() {
            log("kitchen: oven block not registered; the mod stays idle");
            return;
        }
        self.lit_block = resolve_block(LIT_BLOCK_KEY);
        if self.lit_block.is_none() {
            log("kitchen: lit oven block not registered; ovens cook without the visual flip");
        }
        self.load_oven_list();
        register_event_handler(EventKind::BlockPlaced, 0, ON_BLOCK_PLACED);
        register_event_handler(EventKind::ContainerOpened, 0, ON_CONTAINER_OPENED);
        register_event_handler(EventKind::ContainerClosed, 0, ON_CONTAINER_CLOSED);
        // After WorldScheduled = right after the engine's own furnace step.
        register_tick_system(Stage::WorldScheduled, AttachSide::After, 0, TICK_SYSTEM);
    }

    fn handle_event(&mut self, handler_id: u32, payload: &mut EventPayload) -> Outcome {
        match (handler_id, &*payload) {
            (ON_BLOCK_PLACED, EventPayload::BlockPlaced { pos, block })
                if Some(*block) == self.oven_block =>
            {
                // `block_placed.pos` is the multi-cell anchor — the same cell
                // the engine keys the container at.
                if !self.ovens.contains(pos) {
                    self.ovens.push(*pos);
                    self.store_oven_list();
                }
            }
            (ON_CONTAINER_OPENED, EventPayload::ContainerOpened { kind, pos }) => {
                if let ContainerKind::Mod { key } = kind {
                    if key == KIND_KEY {
                        self.open_session = *pos;
                    }
                }
            }
            (ON_CONTAINER_CLOSED, EventPayload::ContainerClosed { kind, .. }) => {
                if matches!(kind, ContainerKind::Mod { key } if key == KIND_KEY) {
                    self.open_session = None;
                }
            }
            _ => {}
        }
        Outcome::Continue
    }

    fn tick_system(&mut self, system_id: u32) {
        if system_id != TICK_SYSTEM || self.ovens.is_empty() {
            return;
        }
        let Some(oven_block) = self.oven_block else {
            return;
        };
        // One batched read prunes stale anchors (broken ovens — reported from
        // ANY footprint cell — decode to a different block at the anchor) and
        // gates on loaded sections in one crossing.
        let positions: Vec<[i32; 3]> = self.ovens.clone();
        let blocks = get_blocks(positions.clone());
        let mut pruned = false;
        for (pos, block) in positions.into_iter().zip(blocks) {
            match block {
                // Unloaded: state is frozen on disk, exactly like a furnace.
                None => continue,
                // A listed anchor is ours in EITHER visual state.
                Some(b) if b == oven_block || Some(b) == self.lit_block => self.step_oven(pos),
                Some(_) => {
                    self.ovens.retain(|p| *p != pos);
                    pruned = true;
                }
            }
        }
        if pruned {
            self.store_oven_list();
        }
    }
}

register_mod!(Kitchen);
