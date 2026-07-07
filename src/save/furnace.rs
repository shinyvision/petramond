//! (De)serialization for furnace MACHINE STATE stored inside a section's save
//! record (burn/cook counters only — a furnace's slots ride the generic
//! container list in `save::container`, and its facing rides the shared
//! entity-facing list; see `save::codec`).

use std::collections::HashMap;

use crate::furnace::Furnace;
use crate::save::codec::{get_indexed, put_indexed, put_u16, Reader};

/// Bytes per serialized furnace: idx(2) + cook/burn_remaining/burn_max (2 each).
const FURNACE_BYTES: usize = 2 + 6;

/// Append a `u16`-length-prefixed list of `(local index, furnace)` records to `buf`.
pub fn put_furnaces(buf: &mut Vec<u8>, furnaces: &HashMap<u16, Furnace>) {
    put_indexed(buf, furnaces, FURNACE_BYTES, |buf, f| {
        put_u16(buf, f.cook_progress);
        put_u16(buf, f.burn_remaining);
        put_u16(buf, f.burn_max);
    });
}

/// Read the furnace list written by [`put_furnaces`]. `None` on truncated input.
pub fn get_furnaces(r: &mut Reader) -> Option<HashMap<u16, Furnace>> {
    get_indexed(r, |r| {
        Some(Furnace {
            cook_progress: r.u16()?,
            burn_remaining: r.u16()?,
            burn_max: r.u16()?,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn furnace_state_roundtrips_through_a_buffer() {
        let mut map = HashMap::new();
        map.insert(
            5u16,
            Furnace {
                cook_progress: 431,
                burn_remaining: 1200,
                burn_max: 4800,
            },
        );
        map.insert(60000u16, Furnace::default());

        let mut buf = Vec::new();
        put_furnaces(&mut buf, &map);
        let mut r = Reader::new(&buf);
        let got = get_furnaces(&mut r).expect("decodes");
        assert_eq!(got, map, "burn/cook state survives the round-trip");
    }

    #[test]
    fn truncated_input_is_none() {
        let mut buf = Vec::new();
        put_u16(&mut buf, 1); // claims one furnace, provides no body
        let mut r = Reader::new(&buf);
        assert!(get_furnaces(&mut r).is_none());
    }
}
