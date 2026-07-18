//! The miller: a hand mill that grinds one input into its `kitchen:milling`
//! product over [`MILL_TICKS`] — no fuel, no heat, just time.
//!
//! Content composition mirrors the oven: the machine, its GUI document
//! (input slot filtered on `kitchen:millable`, take-only output, progress
//! arrow), and the `kitchen:milling` recipe CLASS are kitchen-owned; the
//! grain and the ground product belong to whichever pack ships them (farming
//! adds `wheat -> flour` as one data row and the `kitchen:millable` tag on
//! its wheat — no code anywhere).
//!
//! State model: progress lives in section cell KV at the anchor (u32 LE),
//! resetting to zero whenever there is no valid job (nothing millable, or a
//! blocked output) — a mill holds no heat to lose. The `flour` cube in the
//! authored model shows exactly while the OUTPUT slot holds anything: the
//! full/empty visual is a same-footprint `swap_model_block` flip between the
//! `kitchen:miller` row (hides the cube) and `kitchen:miller_full`, compared
//! against the anchor's CURRENT block each tick so the visual self-heals
//! instead of tracking transitions.

use mod_sdk::*;

use crate::machine::{
    consume_one, merge_output, output_accepts, write_changed_slots, Caches, Machine, MachineSpec,
    StepCtx,
};

const STATE_KEY: &str = "kitchen:mill_state";

/// The miller's machine-processing recipe class (any pack may add rows).
const MILLING_CLASS: &str = "kitchen:milling";

const SLOT_INPUT: usize = 0;
const SLOT_OUTPUT: usize = 1;

/// Game ticks to grind one input item (10 s at 20 TPS).
const MILL_TICKS: u32 = 200;

pub type Miller = Machine<MillerSpec>;

#[derive(Default)]
pub struct MillerSpec;

impl MachineSpec for MillerSpec {
    const KIND_KEY: &'static str = "kitchen:miller";
    const BLOCK_KEY: &'static str = "kitchen:miller";
    /// The output-holding variant: same authored model with the `flour` cube
    /// visible (the empty row lists it in `hidden_parts`).
    const VARIANT_KEY: &'static str = "kitchen:miller_full";
    const ANCHORS_KEY: &'static str = "kitchen:millers";

    /// One miller's game tick. Writes back only what changed. A `None`
    /// container = never opened, never written — but the machine still steps
    /// so the visual self-heals (an emptied-and-broken-elsewhere edge can't
    /// strand the flour cube).
    fn step(
        &mut self,
        ctx: &StepCtx,
        caches: &mut Caches,
        slots: Option<Vec<Option<ItemStackData>>>,
    ) {
        let mut slots = slots.unwrap_or_default();
        slots.resize(2, None);
        let state_bytes = section_kv_get(ctx.pos, STATE_KEY).unwrap_or_default();
        let mut progress = ByteReader::new(&state_bytes).u32().unwrap_or(0);
        let before_progress = progress;
        let before_slots = slots.clone();

        // The job: a millable input whose product fits the output.
        let result = slots[SLOT_INPUT]
            .as_ref()
            .filter(|s| s.count > 0)
            .map(|s| s.item.clone())
            .and_then(|k| caches.recipe_for(MILLING_CLASS, &k));
        let can_mill = result
            .as_ref()
            .is_some_and(|r| output_accepts(caches, &slots[SLOT_OUTPUT], r));

        if can_mill {
            progress += 1;
            if progress >= MILL_TICKS {
                progress = 0;
                let result = result.expect("can_mill implies a result");
                merge_output(&mut slots[SLOT_OUTPUT], &result);
                consume_one(&mut slots[SLOT_INPUT]);
            }
        } else {
            // No heat to preserve: an interrupted grind starts over.
            progress = 0;
        }

        write_changed_slots(ctx.pos, &before_slots, &slots);
        if progress != before_progress {
            section_kv_set(ctx.pos, STATE_KEY, progress.to_le_bytes().to_vec());
        }
        // The flour cube shows exactly while the output holds anything.
        // Compared against the CURRENT block, so the flip is idempotent and
        // self-healing, and still crosses the ABI only on a real mismatch.
        if let Some(full) = ctx.variant {
            let want = if slots[SLOT_OUTPUT].is_some() {
                full
            } else {
                ctx.block
            };
            if ctx.current != want {
                swap_model_block(ctx.pos, want);
            }
        }
        if ctx.gui_open {
            gui_state_set(
                "kitchen:mill01",
                GuiValue::F32(progress as f32 / MILL_TICKS as f32),
            );
        }
    }
}
