//! Wire encoding helpers: postcard encode/decode and the packed ptr/len
//! return lane of `mod_dispatch`/`host_dispatch`.

use serde::{Deserialize, Serialize};

/// Pack a guest-memory buffer address for the `u64` return lane of
/// `mod_dispatch`/`host_dispatch`: `ptr << 32 | len`.
#[inline]
pub fn pack_ptr_len(ptr: u32, len: u32) -> u64 {
    ((ptr as u64) << 32) | len as u64
}

/// Inverse of [`pack_ptr_len`].
#[inline]
pub fn unpack_ptr_len(packed: u64) -> (u32, u32) {
    ((packed >> 32) as u32, packed as u32)
}

/// Encode any ABI value for the wire.
pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, postcard::Error> {
    postcard::to_allocvec(value)
}

/// Decode any ABI value from the wire.
pub fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, postcard::Error> {
    postcard::from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ptr_len_packing_is_lossless() {
        for (ptr, len) in [(0, 0), (1, u32::MAX), (u32::MAX, 17), (0x1234_5678, 9)] {
            assert_eq!(unpack_ptr_len(pack_ptr_len(ptr, len)), (ptr, len));
        }
    }
}
