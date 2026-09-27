use crate::block::Block;
use crate::mathh::IVec3;
use crate::world::data::WorldData;

mod dirt;
mod grass;
mod inert;
mod leaves;
mod wasm;

pub use wasm::BlockHook;

pub use dirt::DIRT;
pub use grass::GRASS;
pub use inert::INERT;
pub use leaves::LEAVES;
pub use leaves::MAX_LOG_DISTANCE;

pub trait BehaviorWorld {
    fn data(&self) -> &WorldData;
    fn queue_block_hook(&mut self, hook: BlockHook);
    fn set_block_world(&mut self, wx: i32, wy: i32, wz: i32, b: Block) -> bool;
    fn break_block_naturally(&mut self, pos: IVec3);
}

pub trait BlockBehavior: Sync {
    fn key(&self) -> &'static str;

    fn has_random_tick(&self) -> bool {
        false
    }

    fn random_tick(&self, world: &mut dyn BehaviorWorld, pos: IVec3) {
        let _ = (world, pos);
    }

    fn neighbor_update(&self, world: &mut dyn BehaviorWorld, pos: IVec3) {
        let _ = (world, pos);
    }

    fn scheduled_tick(&self, world: &mut dyn BehaviorWorld, pos: IVec3) {
        let _ = (world, pos);
    }
}

pub fn by_name(name: &str) -> Option<&'static dyn BlockBehavior> {
    Some(match name {
        "inert" => &INERT,
        "grass" => &GRASS,
        "dirt" => &DIRT,
        "leaves" => &LEAVES,
        "fluid" => &FLUID_HOOK,
        "fragile" => &FRAGILE_HOOK,
        "sapling" => &SAPLING_HOOK,
        "door" => &DOOR_HOOK,
        _ if crate::registry::namespace(name)
            .is_some_and(|ns| ns != crate::registry::ENGINE_NAMESPACE) =>
        {
            wasm::interned(name)
        }
        _ => return None,
    })
}

pub struct EngineHook {
    key: &'static str,
    random_tick: bool,
}

pub static FLUID_HOOK: EngineHook = EngineHook {
    key: "fluid",
    random_tick: false,
};
pub static FRAGILE_HOOK: EngineHook = EngineHook {
    key: "fragile",
    random_tick: false,
};
pub static SAPLING_HOOK: EngineHook = EngineHook {
    key: "sapling",
    random_tick: true,
};
pub static DOOR_HOOK: EngineHook = EngineHook {
    key: "door",
    random_tick: false,
};

impl BlockBehavior for EngineHook {
    fn key(&self) -> &'static str {
        self.key
    }

    fn has_random_tick(&self) -> bool {
        self.random_tick
    }

    fn random_tick(&self, _world: &mut dyn BehaviorWorld, _pos: IVec3) {
        debug_assert!(
            false,
            "engine behaviour '{}' must dispatch through the engine registry",
            self.key
        );
    }

    fn neighbor_update(&self, _world: &mut dyn BehaviorWorld, _pos: IVec3) {
        debug_assert!(
            false,
            "engine behaviour '{}' must dispatch through the engine registry",
            self.key
        );
    }

    fn scheduled_tick(&self, _world: &mut dyn BehaviorWorld, _pos: IVec3) {
        debug_assert!(
            false,
            "engine behaviour '{}' must dispatch through the engine registry",
            self.key
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn behavior_names_round_trip() {
        for name in [
            "inert", "grass", "dirt", "leaves", "fluid", "fragile", "sapling", "door",
        ] {
            let b = by_name(name).unwrap_or_else(|| panic!("unregistered behavior '{name}'"));
            assert_eq!(b.key(), name, "key() must be the inverse of by_name()");
        }
        assert!(by_name("bogus").is_none());
    }
}
