//! Relay mod: a block that carries items from the container above it into the container below it.
//!
//! The block is plain pack data plus the `relay:relay` behavior. Every relay row carries a
//! `relay:transfer` data row (`interval` ticks between moves, `stack` items per move), so a pack
//! adds a faster or slower relay by adding a block row, with no code change.
//!
//! A relay steps on its scheduled tick and schedules the next one, so it needs no world sweep.
//! Placing one starts it; a random tick restarts one whose pending tick was lost. Only the source
//! is read. The move is one `container_transfer`, which the engine applies atomically against the
//! target: it creates the container of a chest nobody opened yet, and a full or picky container
//! just leaves the items where they were. A step that moved nothing waits `idle_interval` ticks
//! (default eight intervals) before it looks again.
//!
//! Known limit: the engine's `take` ignores per-slot take rules, so a relay under a furnace
//! drains its fuel and input slots too, not only the output.

use mod_sdk::*;

mod relay;
#[cfg(test)]
mod testing;
#[cfg(test)]
mod tests;

use relay::{Relay, Transfer};

mod keys {
    mod_sdk::pack_keys! {
        pub RELAY_BEHAVIOR: Behavior = "relay:relay";
        pub RELAY_BLOCK: Block = "relay:relay";
        pub RELAY_ITEM: Item = "relay:relay";
        pub RELAY_RECIPE: Recipe = "relay:relay";
    }
}

const RELAY_HOOK: u32 = 1;
const ON_BLOCK_PLACED: u32 = 1;
const TRANSFER_KEY: &str = "relay:transfer";

impl Mod for Relay {
    fn init(&mut self) {
        *self = Relay::with_rows(blocks_with_data_as::<Transfer>(TRANSFER_KEY));
        if self.is_idle() {
            log("relay: no relay blocks registered; the mod stays idle");
            return;
        }
        register_block_behavior(keys::RELAY_BEHAVIOR, RELAY_HOOK);
        register_event_handler(EventKind::BlockPlaced, 0, ON_BLOCK_PLACED);
    }

    fn handle_event(&mut self, handler_id: u32, payload: &mut EventPayload) -> Outcome {
        if let (ON_BLOCK_PLACED, EventPayload::BlockPlaced { pos, block }) = (handler_id, &*payload)
        {
            self.placed(*pos, *block);
        }
        Outcome::Continue
    }

    fn block_hooks(&mut self, callback_id: u32, hooks: &[(BlockHookKind, [i32; 3])]) {
        if callback_id == RELAY_HOOK {
            self.hooks(hooks);
        }
    }
}

register_mod!(Relay);
