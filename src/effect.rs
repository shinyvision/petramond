//! Player status effects: the effect registry and the timed active state.
//!
//! An effect is a data row in `assets/effects.json` (a layered catalog like
//! `sounds.json`): engine effects own the low ids in the frozen const order
//! below; a mod pack ADDS an effect with a namespaced (`mod_id:name`) key,
//! which registers a fresh id in load order (see [`crate::registry`]).
//!
//! A row's `behavior` names what the engine does while the effect is active
//! (`"regen"` heals on an interval; `"none"` is a pure marker a mod's own tick
//! system can query through the `EffectsActive` host call). The ACTIVE state —
//! which effects the player currently has and for how many more ticks — lives
//! on [`crate::player::Player`] and is stepped once per game tick by
//! `Game::tick_effects` (`src/game/health.rs`), never in per-frame code.
//! Persistence is by registry NAME in `players/<name>.dat` (ids are
//! session-scoped).

use std::sync::LazyLock;

use serde::Deserialize;

/// A status effect kind, identified by its opaque runtime id (the row index in
/// the loaded table). Engine effects own the low ids in the frozen const order
/// below; pack effects register after them.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct Effect(pub u8);

#[allow(non_upper_case_globals)]
impl Effect {
    /// Health regeneration: heals on a fixed tick interval while active.
    pub const Regeneration: Effect = Effect(0);
}

/// Engine effect names in frozen id order (`ENGINE_EFFECT_NAMES[id]` names
/// `Effect(id)`); the completeness oracle `effects.json` is validated against.
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
    /// This effect's definition row.
    #[inline]
    pub fn def(self) -> &'static EffectDef {
        &defs()[self.0 as usize]
    }

    /// Every registered effect (engine + packs), id-ordered.
    pub fn all() -> impl Iterator<Item = Effect> {
        (0..defs().len()).map(|id| Effect(id as u8))
    }
}

/// What the engine does while an effect is active. Pack-registered effects may
/// use `none` and drive their consequences from their own WASM tick system.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum EffectBehavior {
    /// A pure marker: the engine only counts the duration down.
    None,
    /// Heal `amount` half-hearts every `interval` ticks while active.
    /// Boundaries are anchored at EXPIRY (a heal fires whenever `remaining %
    /// interval == 0`, including the expiry tick itself), so the first heal
    /// lands `interval` ticks after application exactly when the granted
    /// duration is a multiple of `interval` — grant such durations.
    Regen { interval: u32, amount: i32 },
}

/// One row of the effect table.
pub struct EffectDef {
    pub effect: Effect,
    /// The row's registry name (`"petramond:regeneration"`, `"mod_id:haste"`) — the
    /// key host calls and `level.dat` persistence resolve through [`by_name`].
    pub name: &'static str,
    /// Human-readable display name — authored row data reserved for a future
    /// HUD tooltip; nothing reads it yet (the icon row is icons-only).
    #[allow(dead_code)]
    pub display: &'static str,
    /// HUD icon, as an asset-relative PNG path resolved through
    /// [`crate::assets`] (pack rows resolve inside their own pack). Expected
    /// 16×16, filling the HUD frame cell; smaller icons composite centered,
    /// oversize icons are nearest-resized.
    pub icon: &'static str,
    pub behavior: EffectBehavior,
}

/// One timed effect on the player: the kind plus its remaining game ticks.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ActiveEffect {
    pub effect: Effect,
    pub remaining: u32,
}

/// One effect row as written in `effects.json`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEffectDef {
    effect: String,
    display: String,
    icon: String,
    behavior: RawBehavior,
}

/// A row's `behavior` as written: `"none"`, or `{"regen": {"interval": ..,
/// "amount": ..}}`. The enum shape gives every behavior its own required
/// params (a missing or misspelled one is a serde error) — adding a behavior
/// is one variant here + one arm in [`RawBehavior::resolve`].
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum RawBehavior {
    None,
    Regen { interval: u32, amount: i32 },
}

impl RawBehavior {
    /// Range-check and convert to the runtime enum.
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
        }
    }
}

#[derive(Deserialize)]
struct RawFile {
    effects: Vec<RawEffectDef>,
}

/// The runtime [`Effect`] registered under `name` (engine `petramond:*` and pack
/// `mod_id:name` keys alike), or `None` when no such row is loaded.
pub fn by_name(name: &str) -> Option<Effect> {
    catalog().id(name).map(Effect)
}

/// The loaded effect table, id-ordered (`defs()[effect.0]`). Loads exactly
/// once; a missing or inconsistent `effects.json` fails loudly at startup.
pub fn defs() -> &'static [EffectDef] {
    catalog().rows()
}

fn catalog() -> &'static crate::registry::Catalog<EffectDef> {
    static TABLE: LazyLock<crate::registry::Catalog<EffectDef>> =
        LazyLock::new(|| crate::registry::read_catalog("effects.json", "effect", parse_layers));
    &TABLE
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
                effect: Effect(id),
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
        // Behavior params ride the behavior object — a stray row-level param
        // is an unknown field, rejected loudly.
        assert!(table(
            r#"{"effects": [{"effect": "petramond:regeneration", "display": "R",
                "icon": "i.png", "behavior": "none", "interval": 5}]}"#
        )
        .is_err());
        // Regen must carry both params (the enum shape requires them)...
        assert!(table(
            r#"{"effects": [{"effect": "petramond:regeneration", "display": "R",
                "icon": "i.png", "behavior": {"regen": {"interval": 100}}}]}"#
        )
        .is_err());
        // ...and they must be positive.
        assert!(table(
            r#"{"effects": [{"effect": "petramond:regeneration", "display": "R",
                "icon": "i.png", "behavior": {"regen": {"interval": 0, "amount": 1}}}]}"#
        )
        .is_err());
        // Unknown behavior names are load errors, not silent markers.
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
