//! The kitchen oven: the engine furnace algorithm over mod container slots,
//! consuming its OWN recipe class (`kitchen:cooking`) — an oven cooks food,
//! it does not smelt ore. See the crate docs for the state model.

use mod_sdk::*;

use crate::machine::{
    consume_one, merge_output, output_accepts, write_changed_slots, Caches, Machine, MachineSpec,
    StepCtx,
};

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
        let mut r = ByteReader::new(bytes);
        OvenState {
            cook_progress: r.u32().unwrap_or(0),
            burn_remaining: r.u32().unwrap_or(0),
            burn_max: r.u32().unwrap_or(0),
        }
    }

    fn encode(self) -> Vec<u8> {
        let mut w = ByteWriter::with_capacity(12);
        w.u32(self.cook_progress);
        w.u32(self.burn_remaining);
        w.u32(self.burn_max);
        w.finish()
    }
}

pub type Oven = Machine<OvenSpec>;

#[derive(Default)]
pub struct OvenSpec;

impl MachineSpec for OvenSpec {
    const KIND_KEY: &'static str = "kitchen:oven";
    const BLOCK_KEY: &'static str = "kitchen:oven";
    /// The lit variant: the same authored model with the `fire` cube visible,
    /// block-light emission, and the underside fire particle emitter — all
    /// pack data on its rows. The spec's only visual job is swapping the
    /// placed block between the two on burn transitions (`swap_model_block`
    /// keeps container + state).
    const VARIANT_KEY: &'static str = "kitchen:oven_lit";
    const ANCHORS_KEY: &'static str = "kitchen:ovens";

    /// One oven's game tick: the engine furnace algorithm over its container
    /// slots. Writes back only what changed. A `None` container = never
    /// opened, never written: an empty oven has nothing to do.
    fn step(
        &mut self,
        ctx: &StepCtx,
        caches: &mut Caches,
        slots: Option<Vec<Option<ItemStackData>>>,
    ) {
        let Some(mut slots) = slots else {
            return;
        };
        slots.resize(3, None);
        let mut state = OvenState::decode(&section_kv_get(ctx.pos, STATE_KEY).unwrap_or_default());
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
            .map(|s| s.item.clone())
            .and_then(|k| caches.recipe_for(COOKING_CLASS, &k));
        let can_cook = result
            .as_ref()
            .is_some_and(|r| output_accepts(caches, &slots[SLOT_OUTPUT], r));

        // Relight from the fuel slot only when the flame is out AND there is
        // cookable work — idle fuel is never consumed (the furnace contract).
        if state.burn_remaining == 0 && can_cook {
            if let Some(fuel) = slots[SLOT_FUEL].clone() {
                let burn = caches.fuel_ticks_for(&fuel.item);
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

        write_changed_slots(ctx.pos, &before_slots, &slots);
        if state != before_state {
            section_kv_set(ctx.pos, STATE_KEY, state.encode());
        }
        // Flip the placed block between the unlit/lit rows on burn transitions
        // only (the swap is engine-idempotent, but there is no reason to cross
        // the ABI 20×/s per oven). The fire cube, glow, and fire particles are
        // all data on the lit row.
        let now_lit = state.burn_remaining > 0;
        if was_lit != now_lit {
            if let Some(lit) = ctx.variant {
                swap_model_block(ctx.pos, if now_lit { lit } else { ctx.block });
            }
        }
        if ctx.gui_open {
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
