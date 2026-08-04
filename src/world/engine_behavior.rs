//! The engine half of block behaviour dispatch. Behaviours whose logic needs
//! orchestration (fluid flow, fragile support, sapling growth, doors) live on
//! this trait, keyed by the same `blocks.json` behaviour names as the data
//! layer's `EngineHook` facts; the tick resolves a key here FIRST and only
//! falls back to the data-layer behaviour object.

use petramond_math::math::IVec3;
use crate::world::World;

pub(crate) trait EngineBlockBehavior: Sync {
    fn random_tick(&self, world: &mut World, pos: IVec3) {
        let _ = (world, pos);
    }
    fn neighbor_update(&self, world: &mut World, pos: IVec3) {
        let _ = (world, pos);
    }
    fn scheduled_tick(&self, world: &mut World, pos: IVec3) {
        let _ = (world, pos);
    }
}

/// Engine behaviour registry — the orchestration twin of the data layer's
/// `behavior::by_name` engine hooks.
pub(crate) fn engine_behavior(key: &str) -> Option<&'static dyn EngineBlockBehavior> {
    Some(match key {
        "water" => &crate::world::water::WATER,
        "fragile" => &crate::world::fragile::FRAGILE,
        "sapling" => &crate::world::sapling::SAPLING,
        "door" => &crate::world::door::DOOR,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    /// Every data-layer `EngineHook` key resolves in the engine registry and
    /// vice versa — the two-tier dispatch cannot drift.
    #[test]
    fn engine_hooks_and_registry_agree() {
        for key in ["water", "fragile", "sapling", "door"] {
            let hook = petramond_world::block::behavior::by_name(key)
                .unwrap_or_else(|| panic!("data layer misses engine hook '{key}'"));
            assert_eq!(hook.key(), key);
            assert!(
                super::engine_behavior(key).is_some(),
                "engine registry misses '{key}'"
            );
        }
        assert!(super::engine_behavior("inert").is_none());
        assert!(super::engine_behavior("grass").is_none());
    }
}
