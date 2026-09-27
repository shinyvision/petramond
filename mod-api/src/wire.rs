use serde::{Deserialize, Serialize};

#[inline]
pub fn pack_ptr_len(ptr: u32, len: u32) -> u64 {
    ((ptr as u64) << 32) | len as u64
}

#[inline]
pub fn unpack_ptr_len(packed: u64) -> (u32, u32) {
    ((packed >> 32) as u32, packed as u32)
}

pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, postcard::Error> {
    postcard::to_allocvec(value)
}

pub fn encode_into<T: Serialize>(value: &T, buf: &mut Vec<u8>) -> Result<usize, postcard::Error> {
    const INITIAL: usize = 1024;
    loop {
        if buf.is_empty() {
            buf.resize(INITIAL, 0);
        }
        match postcard::to_slice(value, buf.as_mut_slice()) {
            Ok(used) => return Ok(used.len()),
            Err(postcard::Error::SerializeBufferFull) => {
                let grown = buf.len() * 2;
                buf.resize(grown, 0);
            }
            Err(e) => return Err(e),
        }
    }
}

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
