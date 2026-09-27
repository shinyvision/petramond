//! Every namespaced (`mod_id:name`) `behavior` row key resolves to this. It is what makes a mod
//! block functional instead of decorative.
//!
//! Hooks can't dispatch inline: behaviors run deep inside `World::game_tick` with no mod host
//! around, and this trait is `Sync` while wasm instances aren't. So hooks get queued as a
//! [`BlockHook`] on the world. The game drains the queue right after the scheduled/random ticks
//! in the same tick and forwards each entry to the owning mod via
//! `ModHost::dispatch_block_hooks`. The handler then edits the world through sim host calls, one
//! dispatch step later than a compiled engine behavior. The ABI contract documents that
//! (`GuestCall::BlockBehavior`).

use std::sync::RwLock;

use mod_api::BlockHookKind;

use super::BehaviorWorld;
use crate::mathh::IVec3;

use super::BlockBehavior;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BlockHook {
    pub kind: BlockHookKind,
    pub key: &'static str,
    pub pos: IVec3,
}

pub struct WasmBehavior {
    key: &'static str,
}

impl BlockBehavior for WasmBehavior {
    fn key(&self) -> &'static str {
        self.key
    }

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

static INTERNED: RwLock<Vec<&'static WasmBehavior>> = RwLock::new(Vec::new());

pub(super) fn interned(key: &str) -> &'static WasmBehavior {
    if let Some(b) = INTERNED.read().unwrap().iter().find(|b| b.key == key) {
        return b;
    }
    let mut table = INTERNED.write().unwrap();
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
mod tests;
