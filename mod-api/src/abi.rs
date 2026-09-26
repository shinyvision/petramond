//! ABI versioning and capability negotiation.
//!
//! Every guest exports the [`AbiVersion`] it was built against and the
//! [`Capabilities`] it cannot run without (the SDK's `register_mod!` emits both
//! from this crate's constants, so a mod never spells them). The host reads
//! them right after instantiation — before `mod_init` runs — and refuses the
//! module with an [`AbiRejection`] instead of letting it decode garbage or trap
//! mid-game. A guest that passes learns the host's version and capability set
//! as `mod_init`'s arguments, so it can branch on optional host features.
//!
//! Past the handshake, a side that receives a call variant it does not know
//! (a newer peer within the same major) answers `Unsupported` instead of
//! trapping: see [`decode_call`], [`HostRet::Unsupported`](crate::HostRet::Unsupported)
//! and [`GuestRet::Unsupported`](crate::GuestRet::Unsupported).

use core::fmt;

use serde::de::{self, DeserializeOwned, Deserializer, Visitor};

/// A mod-ABI revision. The major changes when an existing encoding changes
/// (a variant reordered or reshaped, a struct field added) — guests of another
/// major cannot talk to this host at all. The minor changes when the ABI only
/// grows (variants appended to the call/reply enums, capability bits added), so
/// either side can meet a newer peer of the same major and decline what it does
/// not know.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AbiVersion {
    pub major: u16,
    pub minor: u16,
}

impl AbiVersion {
    /// The `u32` the `mod_abi_version` export and `mod_init` carry:
    /// `major << 16 | minor`.
    pub const fn pack(self) -> u32 {
        (self.major as u32) << 16 | self.minor as u32
    }

    /// Inverse of [`pack`](Self::pack).
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

/// The ABI revision this crate describes — what the engine speaks and what
/// every guest built against this crate declares.
///
/// 2.0 nested [`HostCall`](crate::HostCall) by domain (a call is
/// `[domain][call][fields]` on the wire), replaced the stringly
/// `HostRet::Error` with the typed [`HostRet::Err`](crate::HostRet::Err),
/// added event-subscription filters to `RegisterEventHandler`, and stopped
/// echoing payloads the engine never reads back.
pub const ABI_VERSION: AbiVersion = AbiVersion { major: 2, minor: 0 };

/// A set of optional host feature domains. A guest declares the ones it cannot
/// run without (`Mod::REQUIRES` in the SDK); the host refuses a guest whose
/// requirements it does not cover, and hands every accepted guest its full
/// set so the guest can probe the rest at runtime.
///
/// Bits are never reused or renumbered. A host feature that an older guest
/// must not assume gets the next free bit and a minor [`ABI_VERSION`] bump; a
/// guest built against a newer minor that requires such a bit is refused by an
/// older host with the bit named, instead of failing on its first call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Capabilities(u64);

impl Capabilities {
    pub const NONE: Self = Self(0);
    /// Worldgen hooks: feature/stage-replacement dispatches and the terrain
    /// query calls a detached worldgen instance may make.
    pub const WORLDGEN: Self = Self(1 << 0);
    /// A presentation-only client module (`Client*` calls and dispatches).
    pub const CLIENT_MODULE: Self = Self(1 << 1);
    /// Procedural shape bakes and placement plans.
    pub const SHAPE_BAKES: Self = Self(1 << 2);
    /// Scripted AI nodes in mob brains.
    pub const AI_NODES: Self = Self(1 << 3);
    /// Mod-registered block behaviors.
    pub const BLOCK_BEHAVIORS: Self = Self(1 << 4);
    /// Mod GUIs (manifest GUIs, click dispatch, session state).
    pub const MOD_GUIS: Self = Self(1 << 5);
    /// Cross-instance worldgen memo (`Memo*` calls).
    pub const WORLDGEN_MEMO: Self = Self(1 << 6);
    /// Explicit player addressing (ABI 1.1): every dispatch names its actor
    /// (`ActingPlayer`, `None` for tick systems, block hooks, spawn picks,
    /// `mod_init` and mob actions), the single-player-era calls that act on
    /// "the acting player" refuse an actor-less dispatch, and each of them
    /// has a twin that names its player (`PlayerStateOf`, `GiveItemTo`,
    /// `TeleportPlayer`, `GuiOpenFor`, ...).
    pub const EXPLICIT_PLAYERS: Self = Self(1 << 7);

    /// Every named capability with its display name, in bit order.
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

    /// Wrap raw bits, keeping ones this build has no name for (a newer peer's
    /// capabilities must survive the round trip to be reported).
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// The part of `self` that `available` does not cover.
    pub const fn missing_from(self, available: Self) -> Self {
        Self(self.0 & !available.0)
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Display for Capabilities {
    /// Comma-separated names; bits this build does not know print as
    /// `bit N` so a newer guest's requirement is still identifiable.
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

/// Everything this engine build supports.
pub const HOST_CAPABILITIES: Capabilities = {
    let mut all = Capabilities::NONE;
    let mut i = 0;
    while i < Capabilities::NAMED.len() {
        all = all.union(Capabilities::NAMED[i].0);
        i += 1;
    }
    all
};

/// Why the host refused a guest at load. The `Display` text is the
/// user-facing explanation (it lands next to "mod 'x' disabled").
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AbiRejection {
    /// The module exports no `mod_abi_version`: built by an SDK that predates
    /// ABI versioning, or not a petramond mod at all.
    Unversioned,
    /// Built against an older major than the host speaks.
    TooOld { guest: AbiVersion, host: AbiVersion },
    /// Built against a newer major than the host speaks.
    TooNew { guest: AbiVersion, host: AbiVersion },
    /// Same major, but the guest requires capabilities the host lacks.
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

/// The load-time handshake: may a guest built for `guest` that requires
/// `requires` run on a host speaking `host` with `supported`? Only the major
/// has to match — within a major, a minor difference is bridged by
/// `Unsupported` replies, and the capability check catches what cannot be.
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

/// A decoded call enum, or the index of a variant this build does not know.
#[derive(Clone, Debug, PartialEq)]
pub enum Decoded<T> {
    Known(T),
    /// A variant index past the end of `T`: a newer peer (same major) using a
    /// call this side predates. Answer `Unsupported`, do not trap. For the
    /// nested [`HostCall`](crate::HostCall), `domain` is the known domain
    /// whose call list `variant` runs past (`None` when the domain index
    /// itself is the unknown one, and for the flat enums).
    Unknown {
        domain: Option<u32>,
        variant: u32,
    },
}

/// Decode a flat call enum (`GuestCall` in the guest; the host's nested
/// `HostCall` goes through [`decode_host_call`](crate::decode_host_call)),
/// telling an unknown-but-well-framed variant apart from a malformed buffer.
/// postcard leads an enum with its variant index as a varint, so a payload
/// whose index is past the enum's last variant is a call from a newer ABI
/// minor; anything else that fails to decode is a broken peer.
pub fn decode_call<T: DeserializeOwned>(bytes: &[u8]) -> Result<Decoded<T>, postcard::Error> {
    match crate::decode(bytes) {
        Ok(call) => Ok(Decoded::Known(call)),
        Err(e) => match postcard::take_from_bytes::<u32>(bytes) {
            Ok((variant, _)) if variant as usize >= variant_count::<T>() => {
                Ok(Decoded::Unknown {
                    domain: None,
                    variant,
                })
            }
            _ => Err(e),
        },
    }
}

/// The number of variants of enum `T`, read from the list its derived
/// `Deserialize` hands `deserialize_enum` (0 for a non-enum). Runs only on the
/// failure path of [`decode_call`], so it is not cached.
fn variant_count<T: DeserializeOwned>() -> usize {
    T::deserialize(VariantCounter)
        .err()
        .map_or(0, |found| found.0)
}

/// A deserializer that never produces a value: it only records how many
/// variants the enum being deserialized declares, carried out in the error.
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
        // Varint 16383: far past the last variant, well-framed.
        assert_eq!(
            decode_call::<GuestCall>(&[0xff, 0x7f]).unwrap(),
            Decoded::Unknown {
                domain: None,
                variant: 16383
            }
        );
        // A known variant index with a truncated body is a broken peer.
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
        // A domain index past the last domain.
        assert_eq!(
            decode_host_call(&[0xff, 0x7f]).unwrap(),
            Decoded::Unknown {
                domain: None,
                variant: 16383
            }
        );
        // A known domain (Core = 0) with a call index past its end.
        let core_calls = HostCall::DOMAINS[0].1.len() as u8;
        assert_eq!(
            decode_host_call(&[0, core_calls]).unwrap(),
            Decoded::Unknown {
                domain: Some(0),
                variant: core_calls as u32
            }
        );
        // A known call with a truncated body is a broken peer.
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
