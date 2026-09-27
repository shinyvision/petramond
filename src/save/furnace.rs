use crate::save::codec::{get_indexed, put_indexed, Reader};
use crate::save::wire::{wire_struct, Wire};
use petramond_world::furnace::Furnace;
use petramond_world::section::CellMap;

const FURNACE_BYTES: usize = 2 + 6;

wire_struct!(Furnace {
    cook_progress,
    burn_remaining,
    burn_max,
});

pub fn put_furnaces(buf: &mut Vec<u8>, furnaces: &CellMap<Furnace>) {
    put_indexed(buf, furnaces, FURNACE_BYTES, |buf, f| f.put(buf));
}

pub fn get_furnaces(r: &mut Reader) -> Option<CellMap<Furnace>> {
    get_indexed(r, Furnace::get)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn furnace_state_roundtrips_through_a_buffer() {
        let mut map = CellMap::new();
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
        crate::save::codec::put_u16(&mut buf, 1);
        let mut r = Reader::new(&buf);
        assert!(get_furnaces(&mut r).is_none());
    }
}
