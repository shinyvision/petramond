//! The narrow MUTATION surface over the world's content state — the world KV,
//! the shader environment, block hooks, custom-shape bakes, torch mounts. Reads
//! go through [`World::data`]; every write a caller outside `world` may make
//! is named here, so nothing reaches the deterministic half mutably from
//! outside.

use crate::world::{ServerWorld, World, WorldSide};
use std::collections::{BTreeMap, BTreeSet};

use petramond_math::math::IVec3;
use petramond_world::block::behavior::BlockHook;
use petramond_world::block::Aabb;
use petramond_world::torch::TorchPlacement;

use super::CustomBakeCell;

impl<S: WorldSide> World<S> {
    /// Set one namespaced visual shader parameter. On the server this is the
    /// tick-side mod write; a replica mirrors the replicated value.
    pub fn set_shader_param(&mut self, key: String, value: [f32; 4]) {
        self.data.set_shader_param(key, value);
    }

    /// Install a custom shape's baked collision boxes for `pos` (the bake
    /// pump's answer for a cell in [`drain_custom_bake_dirty`]).
    ///
    /// [`drain_custom_bake_dirty`]: Self::drain_custom_bake_dirty
    pub fn set_custom_bake(&mut self, pos: IVec3, boxes: &[Aabb]) {
        self.data.set_custom_bake(pos, boxes);
    }

    /// Take the custom-shape cells awaiting a (re)bake.
    pub fn drain_custom_bake_dirty(&mut self) -> Vec<CustomBakeCell> {
        self.data.drain_custom_bake_dirty()
    }

    /// Record a freshly placed torch's mount. No-op if the owning section is
    /// not loaded.
    pub fn insert_torch(&mut self, pos: IVec3, placement: TorchPlacement) {
        self.data.insert_torch(pos, placement);
    }

    /// Mark the section owning `pos` modified, so a change no tick would
    /// otherwise re-flag (a GUI edit to an idle chest/furnace) persists.
    pub fn mark_chunk_modified(&mut self, pos: IVec3) {
        self.data.mark_chunk_modified(pos);
    }
}

impl ServerWorld {
    /// Set one entry of the world's persistent key/value map.
    pub fn world_kv_set(&mut self, key: String, value: Vec<u8>) {
        self.data.world_kv_set(key, value);
    }

    /// Remove one world KV entry; returns whether it was present.
    pub fn world_kv_remove(&mut self, key: &str) -> bool {
        self.data.world_kv_remove(key)
    }

    /// Restore the whole world KV map at session open.
    pub fn set_world_kv(&mut self, map: BTreeMap<String, Vec<u8>>) {
        self.data.set_world_kv(map);
    }

    /// Install the world's disabled-mod set — once, at session open.
    pub fn set_disabled_mods(&mut self, disabled: BTreeSet<String>) {
        self.data.set_disabled_mods(disabled);
    }

    /// Drain the mod-behavior hooks fired this tick, in fire order.
    pub fn take_block_hooks(&mut self) -> Vec<BlockHook> {
        self.data.take_block_hooks()
    }
}
