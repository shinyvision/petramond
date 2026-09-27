#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PackKeyKind {
    Block,
    Item,
    Mob,
    Sound,
    Effect,
    Condition,
    Loot,
    Emitter,
    Model,
    Shape,
    Recipe,
    RecipeClass,
    UndergroundBiome,
    ShaderParam,
    Tag,
    MobTag,
    Data,
    Behavior,
    AiNode,
    GuiKind,
    GuiState,
    Widget(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PackKey {
    pub kind: PackKeyKind,
    pub key: &'static str,
}

#[macro_export]
macro_rules! pack_keys {
    ($(
        $(#[$meta:meta])*
        $vis:vis $name:ident : $kind:ident $(($doc:expr))? = $key:literal;
    )*) => {
        $( $(#[$meta])* $vis const $name: &str = $key; )*

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
