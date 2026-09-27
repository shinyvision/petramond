//! Kitchen mod: craftable food machines on the mod container-slot API, one module per machine.
//!
//! - [`oven`]: like the furnace but runs its own `kitchen:cooking` recipe class, so it never smelts
//!   ore. Flips to `kitchen:oven_lit` row while burning (fire cube, glow, particles).
//! - [`vessels`]: bowl left behind after eating, hooked into `ItemUseEvent::Eaten`.
//! - [`oven_draw`]: draws the oven's input and output items each tick via `set_block_draw`.
//! - [`miller`]: grinds input into `kitchen:milling` product every 200 ticks, no fuel. Flips to
//!   `kitchen:miller_full` row while there's output waiting.
//!
//! Both are `MachineSpec`s over `machine-core`, which owns the persisted anchor registries (pruned
//! each tick from one batched block read), session caches, and slot arithmetic.
//!
//! This crate is only the machine logic; blocks/items/recipes/models/GUI are pack data. The recipe
//! classes are the extension point: a pack adds `kitchen:cooking`/`kitchen:milling` rows plus the
//! matching slot-filter tags, no code change needed here or in the engine. Farming pack's
//! dough→bread and wheat→flour recipes work this way.
//!
//! Runs as one tick system right after the engine's `WorldScheduled` window, reading all machine
//! slots via batched calls and writing back only what changed.

mod keys;
mod miller;
mod oven;
mod oven_draw;
mod vessels;

use machine_core::Caches;
use miller::Miller;
use mod_sdk::*;
use oven::Oven;
use vessels::Vessels;

const TICK_SYSTEM: u32 = 1;
const ON_BLOCK_PLACED: u32 = 1;
const ON_CONTAINER_OPENED: u32 = 2;
const ON_ITEM_USED: u32 = 3;

#[derive(Default)]
struct Kitchen {
    oven: Oven,
    miller: Miller,
    caches: Caches,
    vessels: Vessels,
}

impl Mod for Kitchen {
    fn init(&mut self) {
        let oven_ok = self.oven.init();
        let miller_ok = self.miller.init();
        if !oven_ok && !miller_ok {
            log("kitchen: no machine blocks registered; the mod stays idle");
            return;
        }
        self.vessels.init();
        register_event_handler(EventKind::BlockPlaced, 0, ON_BLOCK_PLACED);
        register_event_handler(EventKind::ContainerOpened, 0, ON_CONTAINER_OPENED);
        register_event_handler(EventKind::ItemUsed, 0, ON_ITEM_USED);
        register_tick_system(Stage::WorldScheduled, AttachSide::After, 0, TICK_SYSTEM);
    }

    fn handle_event(&mut self, handler_id: u32, payload: &mut EventPayload) -> Outcome {
        match (handler_id, &*payload) {
            (ON_BLOCK_PLACED, EventPayload::BlockPlaced { pos, block }) => {
                self.oven.on_placed(*pos, *block);
                self.miller.on_placed(*pos, *block);
            }
            (ON_CONTAINER_OPENED, EventPayload::ContainerOpened { kind, at }) => {
                self.oven.on_container_opened(kind, *at);
                self.miller.on_container_opened(kind, *at);
            }
            (ON_ITEM_USED, EventPayload::ItemUsed { player, item, kind }) => {
                self.vessels.on_item_used(*player, *item, *kind)
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
