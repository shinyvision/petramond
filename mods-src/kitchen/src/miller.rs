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

use crate::machine::{consume_one, merge_output, output_accepts, AnchorRegistry, Caches};

pub const KIND_KEY: &str = "kitchen:miller";
const MILLER_BLOCK_KEY: &str = "kitchen:miller";
/// The output-holding variant: same authored model with the `flour` cube
/// visible (the empty row lists it in `hidden_parts`).
const FULL_BLOCK_KEY: &str = "kitchen:miller_full";
const STATE_KEY: &str = "kitchen:mill_state";

/// The miller's machine-processing recipe class (any pack may add rows).
const MILLING_CLASS: &str = "kitchen:milling";

const SLOT_INPUT: usize = 0;
const SLOT_OUTPUT: usize = 1;

/// Game ticks to grind one input item (10 s at 20 TPS).
const MILL_TICKS: u32 = 200;

pub struct Miller {
    block: Option<BlockId>,
    /// `None` degrades to milling with no visual flip, never to not milling.
    full_block: Option<BlockId>,
    anchors: AnchorRegistry,
    /// The miller whose GUI session is open, if any (gauge publish gate).
    open_session: Option<[i32; 3]>,
}

impl Default for Miller {
    fn default() -> Self {
        Miller {
            block: None,
            full_block: None,
            anchors: AnchorRegistry::new("kitchen:millers"),
            open_session: None,
        }
    }
}

impl Miller {
    /// Resolve blocks + restore the anchor list; `false` = the pack's miller
    /// rows are missing and this machine stays idle.
    pub fn init(&mut self) -> bool {
        self.block = resolve_block(MILLER_BLOCK_KEY);
        if self.block.is_none() {
            log("kitchen: miller block not registered; millers stay idle");
            return false;
        }
        self.full_block = resolve_block(FULL_BLOCK_KEY);
        if self.full_block.is_none() {
            log("kitchen: full miller block not registered; millers grind without the visual");
        }
        self.anchors.load();
        true
    }

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
        let full = self.full_block;
        let live = self.anchors.prune_live(|b| b == block || Some(b) == full);
        if live.is_empty() {
            return;
        }
        let positions: Vec<[i32; 3]> = live.iter().map(|(p, _)| *p).collect();
        let containers = container_get_many(positions);
        for ((pos, current), slots) in live.into_iter().zip(containers) {
            // A `None` container = never opened, never written — but the
            // visual still self-heals (an emptied-and-broken-elsewhere edge
            // can't strand the flour cube).
            self.step(caches, pos, current, slots.unwrap_or_default());
        }
    }

    /// One miller's game tick. Writes back only what changed.
    fn step(
        &mut self,
        caches: &mut Caches,
        pos: [i32; 3],
        current_block: BlockId,
        mut slots: Vec<Option<ItemStackData>>,
    ) {
        slots.resize(2, None);
        let state_bytes = section_kv_get(pos, STATE_KEY).unwrap_or_default();
        let mut progress = u32::from_le_bytes(
            state_bytes
                .get(0..4)
                .and_then(|b| b.try_into().ok())
                .unwrap_or([0; 4]),
        );
        let before_progress = progress;
        let before_slots = slots.clone();

        // The job: a millable input whose product fits the output.
        let result = slots[SLOT_INPUT]
            .as_ref()
            .filter(|s| s.count > 0)
            .map(|s| s.key.clone())
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

        if slots != before_slots {
            let writes = (0..2)
                .filter(|&i| slots[i] != before_slots[i])
                .map(|i| (i as u32, slots[i].clone()))
                .collect();
            container_set(pos, writes);
        }
        if progress != before_progress {
            section_kv_set(pos, STATE_KEY, progress.to_le_bytes().to_vec());
        }
        // The flour cube shows exactly while the output holds anything.
        // Compared against the CURRENT block, so the flip is idempotent and
        // self-healing, and still crosses the ABI only on a real mismatch.
        if let (Some(empty), Some(full)) = (self.block, self.full_block) {
            let want = if slots[SLOT_OUTPUT].is_some() {
                full
            } else {
                empty
            };
            if current_block != want {
                swap_model_block(pos, want);
            }
        }
        if self.open_session == Some(pos) {
            gui_state_set(
                "kitchen:mill01",
                GuiValue::F32(progress as f32 / MILL_TICKS as f32),
            );
        }
    }
}
