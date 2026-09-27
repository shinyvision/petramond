use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use mod_api::{AbiVersion, Capabilities};

static HOST_ABI: AtomicU32 = AtomicU32::new(0);
static HOST_CAPS: AtomicU64 = AtomicU64::new(0);

pub(crate) fn record_host(abi: u32, caps: u64) {
    HOST_ABI.store(abi, Ordering::Relaxed);
    HOST_CAPS.store(caps, Ordering::Relaxed);
}

pub fn host_abi() -> AbiVersion {
    AbiVersion::unpack(HOST_ABI.load(Ordering::Relaxed))
}

pub fn host_supports(caps: Capabilities) -> bool {
    Capabilities::from_bits(HOST_CAPS.load(Ordering::Relaxed)).contains(caps)
}
