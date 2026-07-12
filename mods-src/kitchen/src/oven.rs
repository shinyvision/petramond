//! The kitchen oven: the engine furnace algorithm over mod container slots,
//! consuming its OWN recipe class (`kitchen:cooking`) — an oven cooks food,
//! it does not smelt ore. See the crate docs for the state model.

use mod_sdk::*;

use crate::machine::{consume_one, merge_output, output_accepts, AnchorRegistry, Caches};

pub const KIND_KEY: &str = "kitchen:oven";
const OVEN_BLOCK_KEY: &str = "kitchen:oven";
/// The lit variant: the same authored model with the `fire` cube visible,
/// block-light emission, and the underside fire particle emitter — all pack
/// data on its rows. The mod's only job is swapping the placed block between
/// the two on burn transitions (`swap_model_block` keeps container + state).
const LIT_BLOCK_KEY: &str = "kitchen:oven_lit";
const STATE_KEY: &str = "kitchen:state";

/// The oven's machine-processing recipe class (see pack recipes.json rows).
const COOKING_CLASS: &str = "kitchen:cooking";

const SLOT_INPUT: usize = 0;
const SLOT_FUEL: usize = 1;
const SLOT_OUTPUT: usize = 2;

/// Mirrors the engine furnace's pacing (600-tick cook, 2-tick regress).
const COOK_TICKS: u32 = 600;
const COOK_REGRESS: u32 = 2;

/// Per-oven burn/cook state, persisted in section cell KV at the anchor
/// (3×u32 LE) so a lit oven reloads mid-bake exactly like the engine furnace.
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

pub struct Oven {
    block: Option<BlockId>,
    /// `None` degrades to cooking with no visual flip, never to not cooking.
    lit_block: Option<BlockId>,
    anchors: AnchorRegistry,
    /// The oven whose GUI session is open, if any (gauge publish gate).
    open_session: Option<[i32; 3]>,
}

impl Default for Oven {
    fn default() -> Self {
        Oven {
            block: None,
            lit_block: None,
            anchors: AnchorRegistry::new("kitchen:ovens"),
            open_session: None,
        }
    }
}

impl Oven {
    /// Resolve blocks + restore the anchor list; `false` = the pack's oven
    /// rows are missing and this machine stays idle.
    pub fn init(&mut self) -> bool {
        self.block = resolve_block(OVEN_BLOCK_KEY);
        if self.block.is_none() {
            log("kitchen: oven block not registered; ovens stay idle");
            return false;
        }
        self.lit_block = resolve_block(LIT_BLOCK_KEY);
        if self.lit_block.is_none() {
            log("kitchen: lit oven block not registered; ovens cook without the visual flip");
        }
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
        if !matches!(kind, ContainerKind::Mod { key } if key == KIND_KEY) {
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

    pub fn tick(&mut self, caches: &mut Caches) {
        let Some(block) = self.block else {
            return;
        };
        if self.anchors.anchors.is_empty() {
            return;
        }
        let lit = self.lit_block;
        let live = self.anchors.prune_live(|b| b == block || Some(b) == lit);
        if live.is_empty() {
            return;
        }
        // One batched container read for every live oven (never container_get
        // in a loop). A `None` container = never opened, never written: an
        // empty oven has nothing to do.
        let positions: Vec<[i32; 3]> = live.iter().map(|(p, _)| *p).collect();
        let containers = container_get_many(positions);
        for ((pos, _), slots) in live.into_iter().zip(containers) {
            if let Some(slots) = slots {
                self.step(caches, pos, slots);
            }
        }
    }

    /// One oven's game tick: the engine furnace algorithm over its container
    /// slots. Writes back only what changed.
    fn step(&mut self, caches: &mut Caches, pos: [i32; 3], mut slots: Vec<Option<ItemStackData>>) {
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
            .and_then(|k| caches.recipe_for(COOKING_CLASS, &k));
        let can_cook = result
            .as_ref()
            .is_some_and(|r| output_accepts(caches, &slots[SLOT_OUTPUT], r));

        // Relight from the fuel slot only when the flame is out AND there is
        // cookable work — idle fuel is never consumed (the furnace contract).
        if state.burn_remaining == 0 && can_cook {
            if let Some(fuel) = slots[SLOT_FUEL].clone() {
                let burn = caches.fuel_ticks_for(&fuel.key);
                if burn > 0 {
                    state.burn_remaining = burn;
                    state.burn_max = burn;
                    consume_one(&mut slots[SLOT_FUEL]);
                }
            }
        }

        if state.burn_remaining > 0 && can_cook {
            state.cook_progress += 1;
            if state.cook_progress >= COOK_TICKS {
                state.cook_progress = 0;
                let result = result.expect("can_cook implies a result");
                merge_output(&mut slots[SLOT_OUTPUT], &result);
                consume_one(&mut slots[SLOT_INPUT]);
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
            if let (Some(oven), Some(lit)) = (self.block, self.lit_block) {
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
