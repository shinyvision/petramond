mod body;
mod load;

pub use body::{ActiveCondition, BodyConditions, ConditionPulse};
pub(crate) use load::RawPulse;

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConditionId(pub u8);

pub const MAX_STAGES: usize = 4;

const ENGINE_CONDITION_NAMES: &[&str] = &["petramond:burning"];

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Pulse {
    pub amount: i32,
    pub interval: u32,
}

#[derive(Clone, Debug)]
pub struct StageDef {
    pub name: &'static str,
    pub damage: Option<Pulse>,
    pub emitter: Option<&'static str>,
    pub cools_to: Option<u8>,
    pub cools_after: f32,
}

#[derive(Clone, Debug)]
pub struct ConditionDef {
    pub id: ConditionId,
    pub name: &'static str,
    pub stages: &'static [StageDef],
}

impl ConditionDef {
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

pub fn by_name(name: &str) -> Option<ConditionId> {
    catalog().id(name).map(|id| ConditionId(id as u8))
}

pub fn defs() -> &'static [ConditionDef] {
    catalog().rows()
}

pub(crate) static CATALOG: crate::content::Slot<crate::registry::Catalog<ConditionDef>> =
    crate::content::Slot::new(
        crate::content::stage::CONDITIONS,
        &[crate::content::stage::PARTICLE_EMITTERS],
        load_catalog,
    );

fn load_catalog(
    reg: &crate::content::ContentRegistry,
) -> Result<crate::registry::Catalog<ConditionDef>, String> {
    crate::registry::read_catalog(reg.packs(), "conditions.json", "condition", |texts| {
        load::parse_layers(texts, ENGINE_CONDITION_NAMES, true)
    })
}

fn catalog() -> &'static crate::registry::Catalog<ConditionDef> {
    CATALOG.current()
}

#[cfg(any(test, feature = "test-support"))]
pub fn parse_test_catalog(texts: &[&str]) -> Result<&'static [ConditionDef], String> {
    load::parse_layers(texts, &[], false).map(|c| c.rows())
}

#[cfg(test)]
mod tests;
