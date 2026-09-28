use core::fmt;

use serde::de::{self, DeserializeOwned, Deserializer, Visitor};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AbiVersion {
    pub major: u16,
    pub minor: u16,
}

impl AbiVersion {
    pub const fn pack(self) -> u32 {
        (self.major as u32) << 16 | self.minor as u32
    }

    pub const fn unpack(packed: u32) -> Self {
        Self {
            major: (packed >> 16) as u16,
            minor: packed as u16,
        }
    }
}

impl fmt::Display for AbiVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

pub const ABI_VERSION: AbiVersion = AbiVersion { major: 4, minor: 0 };

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Capabilities(u64);

impl Capabilities {
    pub const NONE: Self = Self(0);
    pub const WORLDGEN: Self = Self(1 << 0);
    pub const CLIENT_MODULE: Self = Self(1 << 1);
    pub const SHAPE_BAKES: Self = Self(1 << 2);
    pub const AI_NODES: Self = Self(1 << 3);
    pub const BLOCK_BEHAVIORS: Self = Self(1 << 4);
    pub const MOD_GUIS: Self = Self(1 << 5);
    pub const WORLDGEN_MEMO: Self = Self(1 << 6);
    pub const EXPLICIT_PLAYERS: Self = Self(1 << 7);

    const NAMED: [(Self, &'static str); 8] = [
        (Self::WORLDGEN, "worldgen"),
        (Self::CLIENT_MODULE, "client-module"),
        (Self::SHAPE_BAKES, "shape-bakes"),
        (Self::AI_NODES, "ai-nodes"),
        (Self::BLOCK_BEHAVIORS, "block-behaviors"),
        (Self::MOD_GUIS, "mod-guis"),
        (Self::WORLDGEN_MEMO, "worldgen-memo"),
        (Self::EXPLICIT_PLAYERS, "explicit-players"),
    ];

    pub const fn bits(self) -> u64 {
        self.0
    }

    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn missing_from(self, available: Self) -> Self {
        Self(self.0 & !available.0)
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Display for Capabilities {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return f.write_str("none");
        }
        let mut first = true;
        for bit in 0..u64::BITS {
            let cap = Self(1 << bit);
            if !self.contains(cap) {
                continue;
            }
            if !first {
                f.write_str(", ")?;
            }
            first = false;
            match Self::NAMED.iter().find(|(named, _)| *named == cap) {
                Some((_, name)) => f.write_str(name)?,
                None => write!(f, "bit {bit}")?,
            }
        }
        Ok(())
    }
}

pub const HOST_CAPABILITIES: Capabilities = {
    let mut all = Capabilities::NONE;
    let mut i = 0;
    while i < Capabilities::NAMED.len() {
        all = all.union(Capabilities::NAMED[i].0);
        i += 1;
    }
    all
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AbiRejection {
    Unversioned,
    TooOld {
        guest: AbiVersion,
        host: AbiVersion,
    },
    TooNew {
        guest: AbiVersion,
        host: AbiVersion,
    },
    MissingCapabilities {
        guest: AbiVersion,
        host: AbiVersion,
        missing: Capabilities,
    },
}

impl fmt::Display for AbiRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unversioned => write!(
                f,
                "incompatible mod: it declares no mod ABI version (built with an SDK older \
                 than ABI {ABI_VERSION}, or not a petramond mod); rebuild it against the \
                 current mod-sdk"
            ),
            Self::TooOld { guest, host } => write!(
                f,
                "incompatible mod: built for mod ABI {guest}, but this game speaks ABI \
                 {host}; rebuild the mod against the current mod-sdk or install an \
                 updated version"
            ),
            Self::TooNew { guest, host } => write!(
                f,
                "incompatible mod: built for mod ABI {guest}, newer than this game's ABI \
                 {host}; update the game or install a version of the mod built for ABI \
                 {}.x",
                host.major
            ),
            Self::MissingCapabilities {
                guest,
                host,
                missing,
            } => write!(
                f,
                "incompatible mod: built for mod ABI {guest}, it requires features this \
                 game (ABI {host}) does not provide: {missing}; update the game"
            ),
        }
    }
}

impl std::error::Error for AbiRejection {}

pub fn negotiate(
    guest: AbiVersion,
    requires: Capabilities,
    host: AbiVersion,
    supported: Capabilities,
) -> Result<(), AbiRejection> {
    if guest.major < host.major {
        return Err(AbiRejection::TooOld { guest, host });
    }
    if guest.major > host.major {
        return Err(AbiRejection::TooNew { guest, host });
    }
    let missing = requires.missing_from(supported);
    if !missing.is_empty() {
        return Err(AbiRejection::MissingCapabilities {
            guest,
            host,
            missing,
        });
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq)]
pub enum Decoded<T> {
    Known(T),
    Unknown { domain: Option<u32>, variant: u32 },
}

pub fn decode_call<T: DeserializeOwned>(bytes: &[u8]) -> Result<Decoded<T>, postcard::Error> {
    match crate::decode(bytes) {
        Ok(call) => Ok(Decoded::Known(call)),
        Err(e) => match postcard::take_from_bytes::<u32>(bytes) {
            Ok((variant, _)) if variant as usize >= variant_count::<T>() => Ok(Decoded::Unknown {
                domain: None,
                variant,
            }),
            _ => Err(e),
        },
    }
}

fn variant_count<T: DeserializeOwned>() -> usize {
    T::deserialize(VariantCounter)
        .err()
        .map_or(0, |found| found.0)
}

struct VariantCounter;

#[derive(Debug)]
struct VariantsFound(usize);

impl fmt::Display for VariantsFound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "enum with {} variants", self.0)
    }
}

impl std::error::Error for VariantsFound {}

impl de::Error for VariantsFound {
    fn custom<M: fmt::Display>(_msg: M) -> Self {
        Self(0)
    }
}

impl<'de> Deserializer<'de> for VariantCounter {
    type Error = VariantsFound;

    fn deserialize_any<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, Self::Error> {
        Err(VariantsFound(0))
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        variants: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value, Self::Error> {
        Err(VariantsFound(variants.len()))
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf option unit unit_struct newtype_struct seq tuple
        tuple_struct map struct identifier ignored_any
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{calls, decode_host_call, encode, GuestCall, HostCall};

    const HOST: AbiVersion = AbiVersion { major: 3, minor: 2 };

    #[test]
    fn version_packing_is_lossless() {
        for v in [
            ABI_VERSION,
            HOST,
            AbiVersion {
                major: u16::MAX,
                minor: 0,
            },
        ] {
            assert_eq!(AbiVersion::unpack(v.pack()), v);
        }
        assert_eq!(AbiVersion { major: 1, minor: 2 }.pack(), 0x0001_0002);
    }

    #[test]
    fn same_major_is_accepted_across_minors() {
        for minor in [0, 2, 9] {
            let guest = AbiVersion { major: 3, minor };
            assert_eq!(
                negotiate(guest, Capabilities::NONE, HOST, HOST_CAPABILITIES),
                Ok(())
            );
        }
    }

    #[test]
    fn other_majors_are_refused_in_the_right_direction() {
        let old = AbiVersion { major: 2, minor: 9 };
        let new = AbiVersion { major: 4, minor: 0 };
        assert_eq!(
            negotiate(old, Capabilities::NONE, HOST, HOST_CAPABILITIES),
            Err(AbiRejection::TooOld {
                guest: old,
                host: HOST
            })
        );
        assert_eq!(
            negotiate(new, Capabilities::NONE, HOST, HOST_CAPABILITIES),
            Err(AbiRejection::TooNew {
                guest: new,
                host: HOST
            })
        );
    }

    #[test]
    fn uncovered_requirements_are_refused_and_named() {
        let guest = AbiVersion { major: 3, minor: 5 };
        let future = Capabilities::from_bits(1 << 40);
        let requires = Capabilities::WORLDGEN
            .union(Capabilities::AI_NODES)
            .union(future);
        let supported = Capabilities::WORLDGEN;
        let err = negotiate(guest, requires, HOST, supported).unwrap_err();
        assert_eq!(
            err,
            AbiRejection::MissingCapabilities {
                guest,
                host: HOST,
                missing: Capabilities::AI_NODES.union(future),
            }
        );
        assert!(err.to_string().contains("ai-nodes, bit 40"), "{err}");
        assert_eq!(negotiate(guest, requires, HOST, requires), Ok(()));
    }

    #[test]
    fn host_supports_every_named_capability() {
        for (cap, _) in Capabilities::NAMED {
            assert!(HOST_CAPABILITIES.contains(cap));
        }
        assert_eq!(Capabilities::NONE.to_string(), "none");
    }

    #[test]
    fn decode_call_tells_unknown_variants_from_malformed_bytes() {
        let known = encode(&GuestCall::TickSystem { id: 3 }).unwrap();
        assert_eq!(
            decode_call::<GuestCall>(&known).unwrap(),
            Decoded::Known(GuestCall::TickSystem { id: 3 })
        );
        assert_eq!(
            decode_call::<GuestCall>(&[0xff, 0x7f]).unwrap(),
            Decoded::Unknown {
                domain: None,
                variant: 16383
            }
        );
        let mut truncated = encode(&GuestCall::TickSystem { id: 300 }).unwrap();
        truncated.pop();
        assert!(decode_call::<GuestCall>(&truncated).is_err());
        assert!(decode_call::<GuestCall>(&[]).is_err());
    }

    #[test]
    fn decode_host_call_tells_unknown_calls_at_both_levels() {
        let tick = HostCall::from(calls::CurrentTick);
        let known = encode(&tick).unwrap();
        assert_eq!(decode_host_call(&known).unwrap(), Decoded::Known(tick));
        assert_eq!(
            decode_host_call(&[0xff, 0x7f]).unwrap(),
            Decoded::Unknown {
                domain: None,
                variant: 16383
            }
        );
        let core_calls = HostCall::DOMAINS[0].1.len() as u8;
        assert_eq!(
            decode_host_call(&[0, core_calls]).unwrap(),
            Decoded::Unknown {
                domain: Some(0),
                variant: core_calls as u32
            }
        );
        let mut truncated = encode(&HostCall::from(calls::GetBlock { pos: [1, 2, 3] })).unwrap();
        truncated.pop();
        assert!(decode_host_call(&truncated).is_err());
        assert!(decode_host_call(&[]).is_err());
    }

    #[test]
    fn variant_count_reads_the_derived_enum() {
        #[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq)]
        enum Three {
            A,
            B(u8),
            C { x: u8 },
        }
        assert_eq!(variant_count::<Three>(), 3);
        assert_eq!(variant_count::<u32>(), 0);
        for value in [Three::A, Three::B(1), Three::C { x: 2 }] {
            let bytes = encode(&value).unwrap();
            assert_eq!(decode_call::<Three>(&bytes).unwrap(), Decoded::Known(value));
        }
        assert_eq!(
            decode_call::<Three>(&[3]).unwrap(),
            Decoded::Unknown {
                domain: None,
                variant: 3
            }
        );
    }
}
