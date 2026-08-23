//! The WASM-forwarding behavior every namespaced (`mod_id:name`) `behavior`
//! row key resolves to — how a mod block becomes *functional* instead of
//! decorative.
//!
//! Behaviors fire deep inside `World::game_tick`, where no mod host is
//! reachable (and the trait is `Sync`, while wasm instances are not), so the
//! hooks don't dispatch inline: they enqueue a [`BlockHook`] on the world,
//! and the game drains the queue right after the world's scheduled/random
//! ticks in the same game tick and forwards each entry to the owning mod
//! (`ModHost::dispatch_block_hooks`). The handler then edits the world
//! through sim host calls — one dispatch step later than a compiled engine
//! behavior would, which is the documented ABI contract
//! (`GuestCall::BlockBehavior`).

use std::sync::RwLock;

use mod_api::BlockHookKind;

use super::BehaviorWorld;
use crate::mathh::IVec3;

use super::BlockBehavior;

/// One queued behavior hook, drained per tick in fire order (deterministic:
/// the world tick that enqueues is itself deterministic).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BlockHook {
    pub kind: BlockHookKind,
    /// The `mod_id:name` behavior key the block's row declares — the dispatch
    /// routes on it, so the block id itself doesn't ride along.
    pub key: &'static str,
    pub pos: IVec3,
}

/// A mod-declared behavior: forwards every hook to the world's hook queue
/// under its row key.
pub struct WasmBehavior {
    key: &'static str,
}

impl BlockBehavior for WasmBehavior {
    fn key(&self) -> &'static str {
        self.key
    }

    /// Mod blocks always take random ticks — whether to act on one is the
    /// mod's decision, made in its handler.
    fn has_random_tick(&self) -> bool {
        true
    }

    fn random_tick(&self, world: &mut dyn BehaviorWorld, pos: IVec3) {
        world.queue_block_hook(BlockHook {
            kind: BlockHookKind::RandomTick,
            key: self.key,
            pos,
        });
    }

    fn neighbor_update(&self, world: &mut dyn BehaviorWorld, pos: IVec3) {
        world.queue_block_hook(BlockHook {
            kind: BlockHookKind::NeighborUpdate,
            key: self.key,
            pos,
        });
    }

    fn scheduled_tick(&self, world: &mut dyn BehaviorWorld, pos: IVec3) {
        world.queue_block_hook(BlockHook {
            kind: BlockHookKind::ScheduledTick,
            key: self.key,
            pos,
        });
    }
}

/// The per-key singletons `by_name` hands out: one leaked `WasmBehavior` per
/// distinct namespaced key, cached so every row sharing a key shares the
/// pointer (the block table stores `&'static dyn BlockBehavior`).
static INTERNED: RwLock<Vec<&'static WasmBehavior>> = RwLock::new(Vec::new());

pub(super) fn interned(key: &str) -> &'static WasmBehavior {
    if let Some(b) = INTERNED.read().unwrap().iter().find(|b| b.key == key) {
        return b;
    }
    let mut table = INTERNED.write().unwrap();
    // Re-check under the write lock (two loaders could race past the read).
    if let Some(b) = table.iter().find(|b| b.key == key) {
        return b;
    }
    let b: &'static WasmBehavior = Box::leak(Box::new(WasmBehavior {
        key: Box::leak(key.to_owned().into_boxed_str()),
    }));
    table.push(b);
    b
}

/// Everything this module's relocated tests (in the engine crate) exercise.
/// Test-support builds only; never a public api surface.
#[cfg(any(test, feature = "test-support"))]
pub mod test_exports {
    pub use super::super::by_name;
    #[allow(unused_imports)]
    pub use super::*;
    pub use crate::mathh::IVec3;
    pub use mod_api::BlockHookKind;
}
