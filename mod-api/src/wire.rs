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

/// A call domain's arm index in [`HostCall`](crate::HostCall): a guest sends a call as
/// `[INDEX][the domain enum's own encoding]`, byte-identical to encoding the outer enum, without
/// linking the outer enum's serializer.
pub trait WireDomain: Serialize {
    const INDEX: u32;
}

/// Encodes `call` as `[WireDomain::INDEX][the domain enum's encoding]` into `buf` (grown as
/// needed), the bytes [`HostCall`](crate::HostCall) would encode to; returns the length used.
pub fn encode_call_into<D: WireDomain>(call: &D, buf: &mut Vec<u8>) -> usize {
    const INITIAL: usize = 1024;
    let mut header = [0u8; 5];
    let mut index = D::INDEX;
    let mut header_len = 0;
    loop {
        let byte = (index & 0x7f) as u8;
        index >>= 7;
        header[header_len] = if index == 0 { byte } else { byte | 0x80 };
        header_len += 1;
        if index == 0 {
            break;
        }
    }
    loop {
        if buf.len() < INITIAL {
            buf.resize(INITIAL, 0);
        }
        buf[..header_len].copy_from_slice(&header[..header_len]);
        match postcard::to_slice(call, &mut buf[header_len..]) {
            Ok(used) => return header_len + used.len(),
            Err(postcard::Error::SerializeBufferFull) => {
                let grown = buf.len() * 2;
                buf.resize(grown, 0);
            }
            Err(e) => panic!("encode host call: {e}"),
        }
    }
}

/// What a reply was when it was not the variant the caller decodes.
#[derive(Debug, Clone, PartialEq)]
pub enum ReplyMismatch {
    /// The host refused the call.
    Err(crate::HostError),
    /// The host predates the call.
    Unsupported,
    /// Another reply variant, by wire index.
    Other(u32),
    Malformed,
}

/// The reply's variant index and its payload bytes.
pub fn split_reply(bytes: &[u8]) -> Result<(u32, &[u8]), ReplyMismatch> {
    let mut value = 0u32;
    for (i, &b) in bytes.iter().enumerate().take(5) {
        value |= u32::from(b & 0x7f) << (7 * i);
        if b < 0x80 {
            return Ok((value, &bytes[i + 1..]));
        }
    }
    Err(ReplyMismatch::Malformed)
}

fn mismatch(index: u32, payload: &[u8]) -> ReplyMismatch {
    if index == crate::ret_index::Err {
        match decode::<crate::HostError>(payload) {
            Ok(e) => ReplyMismatch::Err(e),
            Err(_) => ReplyMismatch::Malformed,
        }
    } else if index == crate::ret_index::Unsupported {
        ReplyMismatch::Unsupported
    } else {
        ReplyMismatch::Other(index)
    }
}

/// The payload of reply variant `expected`, decoded as `T`.
pub fn decode_reply<T: for<'de> Deserialize<'de>>(
    bytes: &[u8],
    expected: u32,
) -> Result<T, ReplyMismatch> {
    let (index, payload) = split_reply(bytes)?;
    if index != expected {
        return Err(mismatch(index, payload));
    }
    decode(payload).map_err(|_| ReplyMismatch::Malformed)
}

pub fn decode_unit_reply(bytes: &[u8], expected: u32) -> Result<(), ReplyMismatch> {
    let (index, payload) = split_reply(bytes)?;
    if index != expected {
        return Err(mismatch(index, payload));
    }
    Ok(())
}

/// [`decode_reply`] for a variant whose payload is `serde_bytes`-encoded.
pub fn decode_bytes_reply<T: BytesWire>(bytes: &[u8], expected: u32) -> Result<T, ReplyMismatch> {
    decode_reply::<T::Wire>(bytes, expected).map(T::from_wire)
}

/// The type a `#[serde(with = "serde_bytes")]` field decodes through.
pub trait BytesWire: Sized {
    type Wire: for<'de> Deserialize<'de>;
    fn from_wire(wire: Self::Wire) -> Self;
}

impl BytesWire for Vec<u8> {
    type Wire = serde_bytes::ByteBuf;
    fn from_wire(wire: Self::Wire) -> Self {
        wire.into_vec()
    }
}

impl BytesWire for Option<Vec<u8>> {
    type Wire = Option<serde_bytes::ByteBuf>;
    fn from_wire(wire: Self::Wire) -> Self {
        wire.map(serde_bytes::ByteBuf::into_vec)
    }
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
