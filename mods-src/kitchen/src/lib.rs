//! Kitchen mod: craftable food machines built entirely on the mod
//! container-slot API, one module per machine.
//!
//! - [`oven`] — the kitchen oven: cooks like the engine furnace (fuel below,
//!   food above, take-only output) but consumes its OWN recipe class,
//!   `kitchen:cooking`, so it cooks food and never smelts ore. While burning
//!   it flips to the `kitchen:oven_lit` row (fire cube + glow + particles are
//!   that row's data).
//! - [`miller`] — the miller: grinds one input into its `kitchen:milling`
//!   product every 200 ticks, no fuel. While the output slot holds anything
//!   it flips to the `kitchen:miller_full` row (the authored `flour` cube).
//! - [`machine`] — what they share: the persisted world-KV anchor registries
//!   (self-healingly pruned each tick from ONE batched block read), the
//!   session registry caches, and the slot arithmetic.
//!
//! Content is pack data (blocks/items/recipes/models/GUI documents); this
//! crate is only the machine logic. Both machines follow the same
//! composition rule: the RECIPE CLASSES are the extension surface — any pack
//! adds `kitchen:cooking` / `kitchen:milling` rows (plus the
//! `kitchen:cookable` / `kitchen:millable` slot-filter tags on its items)
//! and the machines pick them up with no code change here or in the engine.
//! The farming pack's dough→bread (oven) and wheat→flour (miller) rows are
//! exactly that.
//!
//! Everything runs on one tick system right after the engine's own
//! `WorldScheduled` window, reading every machine's slots through batched
//! calls (the ABI hot-loop rule) and writing back only what changed.

mod machine;
mod miller;
mod oven;

use mod_sdk::*;

use machine::Caches;
use miller::Miller;
use oven::Oven;

const TICK_SYSTEM: u32 = 1;
const ON_BLOCK_PLACED: u32 = 1;
const ON_CONTAINER_OPENED: u32 = 2;
const ON_CONTAINER_CLOSED: u32 = 3;

#[derive(Default)]
struct Kitchen {
    oven: Oven,
    miller: Miller,
    caches: Caches,
}

impl Mod for Kitchen {
    fn init(&mut self) {
        let oven_ok = self.oven.init();
        let miller_ok = self.miller.init();
        if !oven_ok && !miller_ok {
            log("kitchen: no machine blocks registered; the mod stays idle");
            return;
        }
        register_event_handler(EventKind::BlockPlaced, 0, ON_BLOCK_PLACED);
        register_event_handler(EventKind::ContainerOpened, 0, ON_CONTAINER_OPENED);
        register_event_handler(EventKind::ContainerClosed, 0, ON_CONTAINER_CLOSED);
        // After WorldScheduled = right after the engine's own furnace step.
        register_tick_system(Stage::WorldScheduled, AttachSide::After, 0, TICK_SYSTEM);
    }

    fn handle_event(&mut self, handler_id: u32, payload: &mut EventPayload) -> Outcome {
        match (handler_id, &*payload) {
            (ON_BLOCK_PLACED, EventPayload::BlockPlaced { pos, block }) => {
                self.oven.on_placed(*pos, *block);
                self.miller.on_placed(*pos, *block);
            }
            (ON_CONTAINER_OPENED, EventPayload::ContainerOpened { kind, pos }) => {
                self.oven.on_container(kind, *pos, true);
                self.miller.on_container(kind, *pos, true);
            }
            (ON_CONTAINER_CLOSED, EventPayload::ContainerClosed { kind, pos }) => {
                self.oven.on_container(kind, *pos, false);
                self.miller.on_container(kind, *pos, false);
            }
            _ => {}
        }
        Outcome::Continue
    }

    fn tick_system(&mut self, system_id: u32) {
        if system_id != TICK_SYSTEM {
            return;
        }
        self.oven.tick(&mut self.caches);
        self.miller.tick(&mut self.caches);
    }
}

register_mod!(Kitchen);
