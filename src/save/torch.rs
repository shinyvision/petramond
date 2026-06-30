//! (De)serialization for the torch orientations stored inside a chunk's save
//! record.
//!
//! A torch's only per-instance state is how it is mounted, so each entry is a
//! single byte. Like the furnace/chest codecs this is a helper for the chunk codec
//! (see `save::codec`): a `u16`-length-prefixed list of `(local index, placement)`
//! appended after the other block-entity sections when `FLAG_HAS_TORCHES` is set.

use std::collections::HashMap;

use crate::save::codec::{get_indexed, put_indexed, put_u8, Reader};
use crate::torch::TorchPlacement;

/// Bytes per serialized torch: idx(2) + placement(1).
const TORCH_BYTES: usize = 2 + 1;

/// Append a `u16`-length-prefixed list of `(local index, placement)` records. The
/// list framing lives in [`put_indexed`](crate::save::codec::put_indexed); this
/// owns only the one-byte placement body.
pub fn put_torches(buf: &mut Vec<u8>, torches: &HashMap<u16, TorchPlacement>) {
    put_indexed(buf, torches, TORCH_BYTES, |buf, p| {
        put_u8(buf, p.to_u8());
    });
}

/// Read the torch list written by [`put_torches`]. `None` on truncated input.
pub fn get_torches(r: &mut Reader) -> Option<HashMap<u16, TorchPlacement>> {
    get_indexed(r, |r| Some(TorchPlacement::from_u8(r.u8()?)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::save::codec::put_u16;

    #[test]
    fn torches_roundtrip_through_a_buffer() {
        let mut map = HashMap::new();
        map.insert(5u16, TorchPlacement::East);
        map.insert(60000u16, TorchPlacement::Floor);
        map.insert(123u16, TorchPlacement::North);

        let mut buf = Vec::new();
        put_torches(&mut buf, &map);
        let mut r = Reader::new(&buf);
        let got = get_torches(&mut r).expect("decodes");
        assert_eq!(got, map, "torch orientation survives the round-trip");
    }

    #[test]
    fn empty_list_roundtrips() {
        let mut buf = Vec::new();
        put_torches(&mut buf, &HashMap::new());
        let mut r = Reader::new(&buf);
        assert!(get_torches(&mut r).expect("decodes").is_empty());
    }

    #[test]
    fn truncated_input_is_none() {
        let mut buf = Vec::new();
        put_u16(&mut buf, 1); // claims one torch, provides no body
        let mut r = Reader::new(&buf);
        assert!(get_torches(&mut r).is_none());
    }
}
