//! The WASM-forwarding behavior every namespaced (`mod_id:name`) `behavior`
//! row key resolves to — how a mod block becomes *functional* instead of
//! decorative.
//!
//! Behaviors fire deep inside `World::game_tick`, where no mod host is
//! reachable (and the trait is `Sync`, while wasm instances are not), so the
//! hooks don't dispatch inline: they enqueue a [`ModBlockHook`] on the world,
//! and the game drains the queue right after the world's scheduled/random
//! ticks in the same game tick and forwards each entry to the owning mod
//! (`ModHost::dispatch_block_hooks`). The handler then edits the world
//! through sim host calls — one dispatch step later than a compiled engine
//! behavior would, which is the documented ABI contract
//! (`GuestCall::BlockBehavior`).

use std::sync::RwLock;

use mod_api::BlockHookKind;

use crate::mathh::IVec3;
use crate::world::World;

use super::BlockBehavior;

/// One queued behavior hook, drained per tick in fire order (deterministic:
/// the world tick that enqueues is itself deterministic).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ModBlockHook {
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

    fn random_tick(&self, world: &mut World, pos: IVec3) {
        world.queue_mod_block_hook(ModBlockHook {
            kind: BlockHookKind::RandomTick,
            key: self.key,
            pos,
        });
    }

    fn neighbor_update(&self, world: &mut World, pos: IVec3) {
        world.queue_mod_block_hook(ModBlockHook {
            kind: BlockHookKind::NeighborUpdate,
            key: self.key,
            pos,
        });
    }

    fn scheduled_tick(&self, world: &mut World, pos: IVec3) {
        world.queue_mod_block_hook(ModBlockHook {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespaced_keys_intern_to_shared_singletons_that_enqueue_hooks() {
        let a = super::super::by_name("testmod:zap").expect("namespaced keys resolve");
        let b = super::super::by_name("testmod:zap").expect("stable");
        assert_eq!(a.key(), "testmod:zap", "key() inverts by_name()");
        assert!(
            std::ptr::eq(a as *const _ as *const u8, b as *const _ as *const u8),
            "one singleton per key"
        );
        assert!(a.has_random_tick());
        assert!(
            super::super::by_name("bogus").is_none(),
            "bare unknowns still error"
        );

        let mut world = crate::world::testutil::flat_world();
        let pos = IVec3::new(1, 65, 1);
        a.random_tick(&mut world, pos);
        a.neighbor_update(&mut world, pos);
        let hooks = world.take_mod_block_hooks();
        assert_eq!(hooks.len(), 2);
        assert_eq!(hooks[0].kind, BlockHookKind::RandomTick);
        assert_eq!(hooks[1].kind, BlockHookKind::NeighborUpdate);
        assert_eq!(hooks[0].key, "testmod:zap");
        assert_eq!(hooks[0].pos, pos);
        assert!(world.take_mod_block_hooks().is_empty(), "take drains");
    }
}
