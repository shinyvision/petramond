/// Wire/save `u8` enum macro.
///
/// Generates the enum, `VARIANTS`, `to_u8` and two decoders.
///
/// `try_from_u8` and `TryFrom<u8>` reject unknown bytes. Use them for untrusted input.
///
/// `from_u8` turns an unknown byte into the default variant. Only okay if a bad byte can't hurt
/// anything.
///
/// `from_index` does `v % VARIANTS.len()` over declaration order, so discriminant gaps don't
/// break it.
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
            type Error = u8;

            #[inline]
            fn try_from(v: u8) -> Result<Self, u8> {
                Self::try_from_u8(v).ok_or(v)
            }
        }

        impl $Name {
            #[allow(dead_code)]
            $vis const VARIANTS: &'static [Self] = &[$(Self::$Variant),+];

            #[inline]
            #[allow(dead_code)]
            $vis fn to_u8(self) -> u8 {
                self as u8
            }

            #[inline]
            #[allow(dead_code)]
            $vis fn try_from_u8(v: u8) -> Option<Self> {
                match v {
                    $($val => Some(Self::$Variant),)+
                    _ => None,
                }
            }

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
