use crate::world::ServerWorld;
use petramond_math::math::IVec3;

pub(crate) trait EngineBlockBehavior: Sync {
    fn random_tick(&self, world: &mut ServerWorld, pos: IVec3) {
        let _ = (world, pos);
    }
    fn neighbor_update(&self, world: &mut ServerWorld, pos: IVec3) {
        let _ = (world, pos);
    }
    fn scheduled_tick(&self, world: &mut ServerWorld, pos: IVec3) {
        let _ = (world, pos);
    }
}

pub(crate) fn engine_behavior(key: &str) -> Option<&'static dyn EngineBlockBehavior> {
    Some(match key {
        "fluid" => &crate::world::fluid::FLUID,
        "fragile" => &crate::world::fragile::FRAGILE,
        "sapling" => &crate::world::sapling::SAPLING,
        "door" => &crate::world::door::DOOR,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn engine_hooks_and_registry_agree() {
        for key in ["fluid", "fragile", "sapling", "door"] {
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
