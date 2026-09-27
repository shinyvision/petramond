use super::ContainerMenu;
use crate::world::ServerWorld;

impl ContainerMenu {
    pub fn open_gauges(&self, world: &ServerWorld) -> Vec<(String, f32)> {
        let Some(pos) = self.target.anchor().and_then(|a| a.block()) else {
            return Vec::new();
        };
        let Some(f) = world.furnace_at(pos) else {
            return Vec::new();
        };
        vec![
            (
                "cook01".to_string(),
                f.cook_progress as f32 / petramond_world::furnace::COOK_TICKS as f32,
            ),
            (
                "burn01".to_string(),
                if f.burn_max == 0 {
                    0.0
                } else {
                    f.burn_remaining as f32 / f.burn_max as f32
                },
            ),
        ]
    }
}
