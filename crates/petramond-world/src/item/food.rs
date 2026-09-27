#[derive(Copy, Clone, Debug, PartialEq)]
pub struct FoodDef {
    pub eat_ticks: u32,
    pub effects: &'static [(crate::effect::Effect, u32)],
}
