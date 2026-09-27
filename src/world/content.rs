use crate::world::{ServerWorld, World, WorldSide};
use std::collections::{BTreeMap, BTreeSet};

use petramond_math::math::IVec3;
use petramond_world::block::behavior::BlockHook;
use petramond_world::block::Aabb;
use petramond_world::torch::TorchPlacement;

use super::CustomBakeCell;

impl<S: WorldSide> World<S> {
    pub fn set_shader_param(&mut self, key: String, value: [f32; 4]) {
        self.data.set_shader_param(key, value);
    }

    pub fn set_custom_bake(&mut self, pos: IVec3, boxes: &[Aabb]) {
        self.data.set_custom_bake(pos, boxes);
    }

    pub fn drain_custom_bake_dirty(&mut self) -> Vec<CustomBakeCell> {
        self.data.drain_custom_bake_dirty()
    }

    pub fn insert_torch(&mut self, pos: IVec3, placement: TorchPlacement) {
        self.data.insert_torch(pos, placement);
    }

    pub fn mark_chunk_modified(&mut self, pos: IVec3) {
        self.data.mark_chunk_modified(pos);
    }
}

impl ServerWorld {
    pub fn world_kv_set(&mut self, key: String, value: Vec<u8>) {
        self.data.world_kv_set(key, value);
    }

    pub fn world_kv_remove(&mut self, key: &str) -> bool {
        self.data.world_kv_remove(key)
    }

    pub fn set_world_kv(&mut self, map: BTreeMap<String, Vec<u8>>) {
        self.data.set_world_kv(map);
    }

    pub fn set_disabled_mods(&mut self, disabled: BTreeSet<String>) {
        self.data.set_disabled_mods(disabled);
    }

    pub fn take_block_hooks(&mut self) -> Vec<BlockHook> {
        self.data.take_block_hooks()
    }
}
