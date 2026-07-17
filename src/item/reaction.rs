use super::ItemType;

/// A dropped-item environmental reaction (`"dropped_reaction"` in
/// `items.json`): when this item's DROPPED entity finds its center inside a
/// matching environment cell during deterministic item physics, the whole
/// stack becomes [`result`](Self::result) in place, 1:1 — count, position,
/// velocity, entity identity, age, and pickup state all preserved; only the
/// stack's item kind changes. It fires once per qualifying entity (the
/// transformed row no longer matches), with ONE optional presentation burst
/// and sound per entity, never per item in the stack.
///
/// This is row-owned CONTENT like `food` or `fuel_burn_ticks`: only items
/// whose rows declare a reaction pay the environment check, and the engine
/// interprets the environment vocabulary — packs supply policy as data
/// (flour-in-water is a pack row, not an engine case).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct DroppedReaction {
    pub environment: ReactionEnvironment,
    /// The item the whole stack becomes.
    pub result: ItemType,
    /// A one-shot burst bundle id (`particle_emitters.json`), fired once per
    /// transformed entity at its position.
    pub burst: Option<u8>,
    /// A one-shot sound (`sounds.json`), played once per transformed entity.
    pub sound: Option<crate::audio::Sound>,
}

/// The environments a [`DroppedReaction`] can react to. An engine-interpreted
/// vocabulary (extend with a variant + one predicate arm — see
/// `world::entities`); rows reference variants by snake_case name.
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReactionEnvironment {
    /// The entity's center cell holds any form of water (source, flowing,
    /// or falling).
    Water,
}
