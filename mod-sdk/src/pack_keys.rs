//! Typed pack ids: the one place a mod names a row, sound, GUI binding or
//! widget of its (or the engine's) pack data.
//!
//! A mod talks to pack data through string keys — `blocks.json` rows, sound
//! ids, GUI bind keys, widget ids. The host validates them only at runtime,
//! and a renamed row degrades a feature with nothing but a log line. So every
//! such key is declared ONCE, through [`pack_keys!`], as a `&str` constant
//! the code uses by name; the same macro records each key with the kind of
//! pack declaration it must match, and the mod's `pack-check` test asserts
//! every recorded key against the shipped JSON. A rename on either side then
//! fails a test instead of a playtest.

/// The kind of pack declaration a [`PackKey`] must match — which JSON field
/// declares it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PackKeyKind {
    /// A `blocks.json` row (`"block"`).
    Block,
    /// An `items.json` row (`"item"`).
    Item,
    /// A `mobs.json` row (`"mob"`).
    Mob,
    /// A `sounds.json` row (`"sound"`).
    Sound,
    /// An `effects.json` row (`"effect"`).
    Effect,
    /// A `conditions.json` row (`"condition"`).
    Condition,
    /// A `loot_tables.json` row (`"loot"`).
    Loot,
    /// A `particle_emitters.json` row (`"emitter"`).
    Emitter,
    /// A `models.json` row (`"key"`).
    Model,
    /// A `shapes.json` row (`"key"`).
    Shape,
    /// A `recipes.json` row (`"recipe"`).
    Recipe,
    /// A recipe class some `recipes.json` row belongs to (`"class"`) — the
    /// key a machine asks the recipe table by.
    RecipeClass,
    /// An `underground_biomes.json` row (`"underground_biome"`).
    UndergroundBiome,
    /// A shader parameter a `shaders.json` pipeline lists under `"params"`.
    ShaderParam,
    /// A block or item tag some row lists under `"tags"`.
    Tag,
    /// A mob tag some `mobs.json` row seeds under its `"tags"` map.
    MobTag,
    /// A per-row data key some row carries under its `"data"` map.
    Data,
    /// A block behavior hook some `blocks.json` row names (`"behavior"`).
    Behavior,
    /// A brain node some `mobs.json` row names (`"node"`).
    AiNode,
    /// A GUI document's `"kind"`.
    GuiKind,
    /// A GUI state key some document binds (a value in a `"bind"` map).
    GuiState,
    /// A widget `"id"` in the GUI document of the given kind.
    Widget(&'static str),
}

/// One declared pack id: the key text and the declaration it must match.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PackKey {
    pub kind: PackKeyKind,
    pub key: &'static str,
}

/// Declare pack ids as `&str` constants, one declaration per id.
///
/// ```ignore
/// mod_sdk::pack_keys! {
///     /// The oven's cook-progress gauge.
///     pub COOK01: GuiState = "kitchen:cook01";
///     pub BACK: Widget("forge:forging_furnace_fittings") = "back";
///     pub(crate) GLOWCAP_PINK: Block = "exploration:glowcap_pink";
/// }
/// ```
///
/// Each entry becomes a constant of the given visibility. Under `cfg(test)`
/// the invocation also defines `PACK_KEYS: &[PackKey]` — every entry with
/// its [`PackKeyKind`] — which the mod's pack-validation test hands to
/// `pack_check::assert_declared`.
#[macro_export]
macro_rules! pack_keys {
    ($(
        $(#[$meta:meta])*
        $vis:vis $name:ident : $kind:ident $(($doc:expr))? = $key:literal;
    )*) => {
        $( $(#[$meta])* $vis const $name: &str = $key; )*

        /// Every pack id declared by this invocation, with the kind of pack
        /// declaration it must match.
        #[cfg(test)]
        pub const PACK_KEYS: &[$crate::PackKey] = &[
            $( $crate::PackKey { kind: $crate::PackKeyKind::$kind $(($doc))?, key: $key }, )*
        ];
    };
}

#[cfg(test)]
mod tests {
    use super::{PackKey, PackKeyKind};

    mod keys {
        crate::pack_keys! {
            /// A documented row.
            pub STONE: Block = "petramond:stone";
            pub(crate) BACK: Widget("forge:fittings") = "back";
        }
    }

    #[test]
    fn the_macro_declares_constants_and_records_each_kind() {
        assert_eq!(keys::STONE, "petramond:stone");
        assert_eq!(keys::BACK, "back");
        assert_eq!(
            keys::PACK_KEYS,
            &[
                PackKey {
                    kind: PackKeyKind::Block,
                    key: "petramond:stone"
                },
                PackKey {
                    kind: PackKeyKind::Widget("forge:fittings"),
                    key: "back"
                },
            ]
        );
    }
}
