/// Edible-item data (`"food"` in `items.json`): the held-button eat duration
/// and the status effects granted when the eat completes. Read from an item
/// via [`ItemType::food`]; the eat itself runs on the tick (see
/// `crate::game::item_use`).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct FoodDef {
    /// Game ticks of held secondary button before the item is consumed.
    pub eat_ticks: u32,
    /// `(effect, duration ticks)` granted on being eaten.
    pub effects: &'static [(crate::effect::Effect, u32)],
}
