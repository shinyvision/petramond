use serde::Deserialize;

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct Effect(pub u8);

#[allow(non_upper_case_globals)]
impl Effect {
    pub const Regeneration: Effect = Effect(0);
}

const ENGINE_EFFECT_NAMES: &[&str] = &["petramond:regeneration"];

impl std::fmt::Debug for Effect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match ENGINE_EFFECT_NAMES.get(self.0 as usize) {
            Some(name) => write!(f, "Effect({name})"),
            None => write!(f, "Effect(#{})", self.0),
        }
    }
}

impl Effect {
    #[inline]
    pub fn def(self) -> &'static EffectDef {
        &defs()[self.0 as usize]
    }

    pub fn all() -> impl Iterator<Item = Effect> {
        (0..defs().len()).map(|id| Effect(id as u8))
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum EffectBehavior {
    None,
    Regen { interval: u32, amount: i32 },
    Speed { scale: f32 },
}

impl EffectBehavior {
    #[inline]
    pub fn speed_scale(self) -> f32 {
        match self {
            Self::Speed { scale } => scale,
            _ => 1.0,
        }
    }
}

pub struct EffectDef {
    pub effect: Effect,
    pub name: &'static str,
    #[allow(dead_code)]
    pub display: &'static str,
    pub icon: &'static str,
    pub behavior: EffectBehavior,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ActiveEffect {
    pub effect: Effect,
    pub remaining: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEffectDef {
    effect: String,
    display: String,
    icon: String,
    behavior: RawBehavior,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum RawBehavior {
    None,
    Regen { interval: u32, amount: i32 },
    Speed { scale: f32 },
}

const SPEED_SCALE_MAX: f32 = 5.0;

impl RawBehavior {
    fn resolve(&self, effect: &str) -> Result<EffectBehavior, String> {
        match *self {
            RawBehavior::None => Ok(EffectBehavior::None),
            RawBehavior::Regen { interval, amount } => {
                if interval == 0 || amount <= 0 {
                    return Err(format!(
                        "effect '{effect}': regen interval and amount must be positive"
                    ));
                }
                Ok(EffectBehavior::Regen { interval, amount })
            }
            RawBehavior::Speed { scale } => {
                if !scale.is_finite() || scale <= 0.0 || scale > SPEED_SCALE_MAX {
                    return Err(format!(
                        "effect '{effect}': speed scale must be in (0, {SPEED_SCALE_MAX}], got {scale}"
                    ));
                }
                Ok(EffectBehavior::Speed { scale })
            }
        }
    }
}

#[derive(Deserialize)]
struct RawFile {
    effects: Vec<RawEffectDef>,
}

pub fn by_name(name: &str) -> Option<Effect> {
    catalog().id(name).map(|id| Effect(id as u8))
}

pub fn defs() -> &'static [EffectDef] {
    catalog().rows()
}

pub(crate) static CATALOG: crate::content::Slot<crate::registry::Catalog<EffectDef>> =
    crate::content::Slot::new(crate::content::stage::EFFECTS, &[], load);

fn load(
    reg: &crate::content::ContentRegistry,
) -> Result<crate::registry::Catalog<EffectDef>, String> {
    crate::registry::read_catalog(reg.packs(), "effects.json", "effect", parse_layers)
}

fn catalog() -> &'static crate::registry::Catalog<EffectDef> {
    CATALOG.current()
}

fn parse_layers(texts: &[&str]) -> Result<crate::registry::Catalog<EffectDef>, String> {
    crate::registry::load_catalog(
        texts,
        |text| serde_json::from_str::<RawFile>(text).map(|f| f.effects),
        |r| &r.effect,
        ENGINE_EFFECT_NAMES,
        "effect",
        |r, id, names| {
            let behavior = r.behavior.resolve(&r.effect)?;
            if r.icon.is_empty() {
                return Err(format!("effect '{}': icon path is empty", r.effect));
            }
            Ok(EffectDef {
                effect: Effect(id as u8),
                name: names.name(id).expect("id resolved from this table"),
                display: Box::leak(r.display.into_boxed_str()),
                icon: Box::leak(r.icon.into_boxed_str()),
                behavior,
            })
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(json: &str) -> Result<crate::registry::Catalog<EffectDef>, String> {
        parse_layers(&[json])
    }

    #[test]
    fn engine_row_holds_its_frozen_id_and_pack_rows_register_after() {
        let base = r#"{"effects": [{"effect": "petramond:regeneration", "display": "Regeneration",
            "icon": "textures/gui/effects/regeneration.png",
            "behavior": {"regen": {"interval": 100, "amount": 1}}}]}"#;
        let pack = r#"{"effects": [{"effect": "mymod:haste", "display": "Haste",
            "icon": "textures/haste.png", "behavior": "none"}]}"#;
        let defs = parse_layers(&[base, pack]).expect("loads").rows();
        assert_eq!(defs[0].name, "petramond:regeneration");
        assert_eq!(defs[0].effect, Effect::Regeneration);
        assert_eq!(
            defs[0].behavior,
            EffectBehavior::Regen {
                interval: 100,
                amount: 1
            }
        );
        assert_eq!(defs[1].name, "mymod:haste");
        assert_eq!(defs[1].behavior, EffectBehavior::None);
    }

    #[test]
    fn behavior_params_are_validated() {
        assert!(table(
            r#"{"effects": [{"effect": "petramond:regeneration", "display": "R",
                "icon": "i.png", "behavior": "none", "interval": 5}]}"#
        )
        .is_err());
        assert!(table(
            r#"{"effects": [{"effect": "petramond:regeneration", "display": "R",
                "icon": "i.png", "behavior": {"regen": {"interval": 100}}}]}"#
        )
        .is_err());
        assert!(table(
            r#"{"effects": [{"effect": "petramond:regeneration", "display": "R",
                "icon": "i.png", "behavior": {"regen": {"interval": 0, "amount": 1}}}]}"#
        )
        .is_err());
        assert!(table(
            r#"{"effects": [{"effect": "petramond:regeneration", "display": "R",
                "icon": "i.png", "behavior": "sparkle"}]}"#
        )
        .is_err());
    }

    #[test]
    fn missing_engine_row_is_a_load_error() {
        assert!(table(r#"{"effects": []}"#).is_err());
    }
}
