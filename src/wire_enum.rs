//! Declarative single-byte wire/save enums. The discriminants ARE the wire and
//! save format, so they are spelled once at the definition and both byte
//! conversions are generated from that same list — a hand-rolled `to_u8`/
//! `from_u8` pair cannot drift from the variants. A neutral leaf module.

/// Declare a `#[repr(u8)]` closed enum whose byte form crosses the wire or the
/// save codec.
///
/// Generates the enum (deriving `Copy, Clone, Debug, PartialEq, Eq`; extra
/// derives/attributes written above the enum pass through), `to_u8` (the
/// discriminant), `from_u8` (exact discriminants; every unknown byte falls
/// back to the declared `default` variant, which also becomes the `Default`
/// impl). `with from_index` additionally generates
/// `from_index(v) = from_u8(v % variant_count)` for cycling selectors.
macro_rules! wire_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $Name:ident: u8 {
            $($(#[$vmeta:meta])* $Variant:ident = $val:literal),+ $(,)?
        }
        default $Default:ident
    ) => {
        $crate::wire_enum::wire_enum!(@base
            $(#[$meta])* $vis enum $Name {
                $($(#[$vmeta])* $Variant = $val),+
            } default $Default
        );
    };
    (
        $(#[$meta:meta])*
        $vis:vis enum $Name:ident: u8 {
            $($(#[$vmeta:meta])* $Variant:ident = $val:literal),+ $(,)?
        }
        default $Default:ident with from_index
    ) => {
        $crate::wire_enum::wire_enum!(@base
            $(#[$meta])* $vis enum $Name {
                $($(#[$vmeta])* $Variant = $val),+
            } default $Default
        );

        impl $Name {
            /// [`from_u8`](Self::from_u8) of `index` wrapped modulo the
            /// variant count, so any counter cycles through every variant.
            #[inline]
            #[allow(dead_code)]
            $vis fn from_index(index: u8) -> Self {
                Self::from_u8(index % ([$($val),+].len() as u8))
            }
        }
    };
    (@base
        $(#[$meta:meta])*
        $vis:vis enum $Name:ident {
            $($(#[$vmeta:meta])* $Variant:ident = $val:literal),+
        }
        default $Default:ident
    ) => {
        $(#[$meta])*
        #[repr(u8)]
        #[derive(Copy, Clone, Debug, PartialEq, Eq)]
        $vis enum $Name {
            $($(#[$vmeta])* $Variant = $val),+
        }

        impl Default for $Name {
            #[inline]
            fn default() -> Self {
                Self::$Default
            }
        }

        impl $Name {
            /// The stable wire/save discriminant.
            #[inline]
            #[allow(dead_code)]
            $vis fn to_u8(self) -> u8 {
                self as u8
            }

            /// Inverse of [`to_u8`](Self::to_u8); unknown bytes fall back to
            /// the declared default variant.
            #[inline]
            #[allow(dead_code)]
            $vis fn from_u8(v: u8) -> Self {
                match v {
                    $($val => Self::$Variant,)+
                    _ => Self::$Default,
                }
            }
        }
    };
}
pub(crate) use wire_enum;
