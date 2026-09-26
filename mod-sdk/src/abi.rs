//! The guest half of ABI negotiation. [`register_mod!`](crate::register_mod)
//! exports the SDK's [`ABI_VERSION`](crate::ABI_VERSION) and the mod's
//! [`Mod::REQUIRES`](crate::Mod::REQUIRES); the host refuses the module at
//! load if they don't fit, and otherwise passes its own version and
//! capabilities into `mod_init`, recorded here for [`host_abi`] and
//! [`host_supports`].

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use mod_api::{AbiVersion, Capabilities};

static HOST_ABI: AtomicU32 = AtomicU32::new(0);
static HOST_CAPS: AtomicU64 = AtomicU64::new(0);

/// Record the host's side of the handshake (`mod_init`'s arguments).
pub(crate) fn record_host(abi: u32, caps: u64) {
    HOST_ABI.store(abi, Ordering::Relaxed);
    HOST_CAPS.store(caps, Ordering::Relaxed);
}

/// The ABI revision of the engine running this mod. Its major always equals
/// the SDK's [`ABI_VERSION`](crate::ABI_VERSION) major (the host refuses
/// anything else); its minor may be older or newer. Valid from
/// [`Mod::init`](crate::Mod::init) on — before that (or off-wasm) it reads
/// `0.0`.
pub fn host_abi() -> AbiVersion {
    AbiVersion::unpack(HOST_ABI.load(Ordering::Relaxed))
}

/// Whether the engine provides every capability in `caps`. Declare the
/// capabilities a mod cannot run without in
/// [`Mod::REQUIRES`](crate::Mod::REQUIRES) (the host then refuses to load it
/// where they are missing); probe optional ones here and skip the feature
/// instead of calling into a host that would answer
/// [`HostRet::Unsupported`](crate::HostRet::Unsupported). Valid from
/// [`Mod::init`](crate::Mod::init) on — before that (or off-wasm) the host is
/// assumed to support nothing.
pub fn host_supports(caps: Capabilities) -> bool {
    Capabilities::from_bits(HOST_CAPS.load(Ordering::Relaxed)).contains(caps)
}
