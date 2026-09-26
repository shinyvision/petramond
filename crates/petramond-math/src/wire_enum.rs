//! Declarative single-byte wire/save enums. The discriminants ARE the wire and
//! save format, so they are spelled once at the definition and both byte
//! conversions are generated from that same list — a hand-rolled `to_u8`/
//! `from_u8` pair cannot drift from the variants. A neutral leaf module.

/// Declare a `#[repr(u8)]` closed enum whose byte form crosses the wire or the
/// save codec.
///
/// Generates the enum (deriving `Copy, Clone, Debug, PartialEq, Eq`; extra
/// derives/attributes written above the enum pass through), `VARIANTS` (every
/// variant in declaration order), `to_u8` (the discriminant), and two
/// decoders a caller chooses between explicitly:
///
/// - `try_from_u8` (and the equivalent `TryFrom<u8>`, whose error is the
///   rejected byte): exact discriminants only, `None` for a byte no variant
///   declares — the decoder for untrusted input, which must fail (or log and
///   substitute) rather than guess.
/// - `from_u8`: the lenient form, mapping an unknown byte to the declared
///   `default` variant (which is also the `Default` impl). Only for bytes
///   whose corruption is harmless to paper over.
///
/// `with from_index` additionally generates `from_index(v)`: the variant at
/// position `v % VARIANTS.len()` in declaration order, for cycling selectors.
/// It indexes the variant list, not the discriminants, so it is correct for
/// non-contiguous discriminants too.
#[macro_export]
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
            /// The variant at `index` modulo the variant count, in
            /// declaration order, so any counter cycles through every
            /// variant whatever the discriminants are.
            #[inline]
            #[allow(dead_code)]
            $vis fn from_index(index: u8) -> Self {
                Self::VARIANTS[usize::from(index) % Self::VARIANTS.len()]
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

        impl TryFrom<u8> for $Name {
            /// The byte no variant declares.
            type Error = u8;

            #[inline]
            fn try_from(v: u8) -> Result<Self, u8> {
                Self::try_from_u8(v).ok_or(v)
            }
        }

        impl $Name {
            /// Every variant, in declaration order.
            #[allow(dead_code)]
            $vis const VARIANTS: &'static [Self] = &[$(Self::$Variant),+];

            /// The stable wire/save discriminant.
            #[inline]
            #[allow(dead_code)]
            $vis fn to_u8(self) -> u8 {
                self as u8
            }

            /// Inverse of [`to_u8`](Self::to_u8): `None` for a byte no
            /// variant declares (corrupt, or from a newer format).
            #[inline]
            #[allow(dead_code)]
            $vis fn try_from_u8(v: u8) -> Option<Self> {
                match v {
                    $($val => Some(Self::$Variant),)+
                    _ => None,
                }
            }

            /// Lenient inverse of [`to_u8`](Self::to_u8): an unknown byte
            /// falls back to the declared default variant. Decoders of
            /// untrusted bytes use [`try_from_u8`](Self::try_from_u8).
            #[inline]
            #[allow(dead_code)]
            $vis fn from_u8(v: u8) -> Self {
                Self::try_from_u8(v).unwrap_or(Self::$Default)
            }
        }
    };
}
pub use wire_enum;

#[cfg(test)]
mod tests {
    wire_enum! {
        enum Sparse: u8 {
            A = 0,
            B = 3,
            C = 7,
        }
        default B with from_index
    }

    wire_enum! {
        enum Dense: u8 {
            X = 0,
            Y = 1,
        }
        default X
    }

    #[test]
    fn bytes_round_trip_through_every_variant() {
        for &v in Sparse::VARIANTS {
            assert_eq!(Sparse::try_from_u8(v.to_u8()), Some(v));
            assert_eq!(Sparse::from_u8(v.to_u8()), v);
            assert_eq!(Sparse::try_from(v.to_u8()), Ok(v));
        }
        assert_eq!(Dense::VARIANTS, &[Dense::X, Dense::Y]);
        assert_eq!(Dense::default(), Dense::X);
    }

    #[test]
    fn unknown_bytes_are_rejected_or_defaulted_explicitly() {
        for byte in [1u8, 2, 4, 8, 255] {
            assert_eq!(Sparse::try_from_u8(byte), None);
            assert_eq!(Sparse::try_from(byte), Err(byte));
            assert_eq!(Sparse::from_u8(byte), Sparse::B);
        }
        assert_eq!(Dense::try_from_u8(2), None);
        assert_eq!(Dense::from_u8(2), Dense::X);
    }

    #[test]
    fn from_index_cycles_declaration_order_not_discriminants() {
        let cycled: Vec<Sparse> = (0..7).map(Sparse::from_index).collect();
        assert_eq!(
            cycled,
            [
                Sparse::A,
                Sparse::B,
                Sparse::C,
                Sparse::A,
                Sparse::B,
                Sparse::C,
                Sparse::A,
            ]
        );
        assert_eq!(Sparse::from_index(255), Sparse::VARIANTS[255 % 3]);
    }
}
