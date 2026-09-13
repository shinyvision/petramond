//! Body conditions: the layered `conditions.json` catalog of timed states a
//! damageable body can carry (burning is one row), and the per-body state that
//! advances them on the fixed tick.
//!
//! A condition is NOT a status effect: it never appears in the active effects
//! list, the effects HUD or the save. A row declares ordered STAGES (row order
//! is strength order); each stage may pulse damage, attach an emitter bundle,
//! and cool to a weaker stage part-way through the ticks it was granted.
//! Engine rows own the low ids in the frozen order below; a pack adds a
//! condition with a namespaced key.

use std::sync::LazyLock;

mod body;
mod load;

pub use body::{ActiveCondition, BodyConditions, ConditionPulse};
pub(crate) use load::RawPulse;

/// A condition kind: its session-scoped row index.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConditionId(pub u8);

/// Most stages one condition row may declare.
pub const MAX_STAGES: usize = 4;

/// Engine condition names in frozen id order.
const ENGINE_CONDITION_NAMES: &[&str] = &["petramond:burning"];

/// Damage dealt every `interval` fixed ticks.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Pulse {
    pub amount: i32,
    pub interval: u32,
}

/// One stage of a condition row.
#[derive(Clone, Debug)]
pub struct StageDef {
    pub name: &'static str,
    pub damage: Option<Pulse>,
    /// A `particle_emitters.json` bundle key shown on the body at this stage.
    pub emitter: Option<&'static str>,
    /// The weaker stage this one steps down to once `cools_after` of its
    /// granted ticks have passed.
    pub cools_to: Option<u8>,
    /// Fraction `(0, 1]` of the granted ticks spent at this stage before
    /// cooling; `1.0` without `cools_to`.
    pub cools_after: f32,
}

/// One loaded condition row.
#[derive(Clone, Debug)]
pub struct ConditionDef {
    pub id: ConditionId,
    pub name: &'static str,
    pub stages: &'static [StageDef],
}

impl ConditionDef {
    /// The index of the stage named `name`.
    pub fn stage(&self, name: &str) -> Option<u8> {
        self.stages
            .iter()
            .position(|s| s.name == name)
            .map(|i| i as u8)
    }
}

impl ConditionId {
    #[inline]
    pub fn def(self) -> &'static ConditionDef {
        &defs()[self.0 as usize]
    }
}

/// The condition registered under `name`.
pub fn by_name(name: &str) -> Option<ConditionId> {
    catalog().id(name).map(|id| ConditionId(id as u8))
}

/// The loaded condition table, id-ordered.
pub fn defs() -> &'static [ConditionDef] {
    catalog().rows()
}

fn catalog() -> &'static crate::registry::Catalog<ConditionDef> {
    static TABLE: LazyLock<crate::registry::Catalog<ConditionDef>> = LazyLock::new(|| {
        crate::registry::read_catalog("conditions.json", "condition", |texts| {
            load::parse_layers(texts, ENGINE_CONDITION_NAMES, true)
        })
    });
    &TABLE
}

/// Parse synthetic layers without engine names or emitter validation.
#[cfg(any(test, feature = "test-support"))]
pub fn parse_test_catalog(texts: &[&str]) -> Result<&'static [ConditionDef], String> {
    load::parse_layers(texts, &[], false).map(|c| c.rows())
}

#[cfg(test)]
mod tests;
