use mod_sdk::*;

use machine_core::{
    consume_one, merge_output, output_accepts, write_changed_slots, Burner, Caches, Machine,
    MachineSpec, Presentation, StepCtx,
};

use crate::keys;

const STATE_KEY: &str = "kitchen:state";

const SLOT_INPUT: usize = 0;
const SLOT_FUEL: usize = 1;
const SLOT_OUTPUT: usize = 2;

const COOK_TICKS: u32 = 600;
const COOK_REGRESS: u32 = 2;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct OvenState {
    cook_progress: u32,
    fire: Burner,
}

const LEGACY_STATE_LEN: usize = 12;

impl KvRecord for OvenState {
    const VERSION: u8 = 1;
    const OLDEST_VERSION: u8 = 0;

    fn encode(&self) -> Vec<u8> {
        let mut w = ByteWriter::with_capacity(LEGACY_STATE_LEN);
        w.u32(self.cook_progress);
        self.fire.encode(&mut w);
        w.finish()
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != LEGACY_STATE_LEN {
            return None;
        }
        let mut r = ByteReader::new(bytes);
        Some(OvenState {
            cook_progress: r.u32()?,
            fire: Burner::decode(&mut r),
        })
    }

    fn upgrade(from: u8, bytes: &[u8]) -> Option<Vec<u8>> {
        (from == 0).then(|| bytes.to_vec())
    }
}

impl OvenState {
    fn load(bytes: &[u8]) -> OvenState {
        if bytes.is_empty() {
            return OvenState::default();
        }
        decode_versioned_or_legacy(bytes, LEGACY_STATE_LEN).unwrap_or_default()
    }
}

pub type Oven = Machine<OvenSpec>;

#[derive(Default)]
pub struct OvenSpec;

impl MachineSpec for OvenSpec {
    const KIND_KEY: &'static str = keys::OVEN_GUI;
    const BLOCK_KEY: &'static str = keys::OVEN_BLOCK;
    const VARIANT_KEYS: &'static [&'static str] = &[keys::OVEN_LIT_BLOCK];
    const ANCHORS_KEY: &'static str = "kitchen:ovens";
    const STATE_KEY: &'static str = STATE_KEY;

    fn step(
        &mut self,
        ctx: &StepCtx<'_>,
        caches: &mut Caches,
        slots: Option<Vec<Option<ItemStackData>>>,
        stored: &mut Vec<u8>,
        out: &mut Presentation,
    ) {
        let Some(mut slots) = slots else {
            return;
        };
        slots.resize(3, None);
        let mut state = OvenState::load(stored);
        let before_state = state;
        let before_slots = slots.clone();

        let was_lit = state.fire.lit();
        state.fire.tick();

        let result = slots[SLOT_INPUT]
            .as_ref()
            .filter(|s| s.count > 0)
            .map(|s| s.item.clone())
            .and_then(|k| caches.recipe_for(keys::COOKING_CLASS, &k));
        let can_cook = result
            .as_ref()
            .is_some_and(|r| output_accepts(caches, &slots[SLOT_OUTPUT], r));

        state.fire.relight(can_cook, &mut slots[SLOT_FUEL], caches);

        if state.fire.lit() && can_cook {
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
            *stored = encode_versioned(&state);
        }
        let now_lit = state.fire.lit();
        if was_lit != now_lit {
            if let Some(lit) = ctx.variant(0) {
                swap_block(ctx.pos, if now_lit { lit } else { ctx.block });
            }
        }
        out.draw(
            ctx.pos,
            crate::oven_draw::contents(
                slots[SLOT_INPUT].as_ref(),
                slots[SLOT_OUTPUT].as_ref(),
                state.cook_progress as f32 / COOK_TICKS as f32,
            ),
        );
        if ctx.gui_open() {
            ctx.publish(
                keys::COOK01,
                GuiValue::F32(state.cook_progress as f32 / COOK_TICKS as f32),
            );
            ctx.publish(keys::BURN01, GuiValue::F32(state.fire.gauge01()));
        }
    }
}
