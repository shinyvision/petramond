use mod_sdk::*;
use weather_core::advance_offset;

const KV_OFF: &str = "weather:off";

#[derive(Clone, Copy, Debug, PartialEq)]
struct Stored([f64; 2]);

const LEGACY_LEN: usize = 16;

impl KvRecord for Stored {
    const VERSION: u8 = 1;
    const OLDEST_VERSION: u8 = 0;

    fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(16);
        bytes.extend_from_slice(&self.0[0].to_le_bytes());
        bytes.extend_from_slice(&self.0[1].to_le_bytes());
        bytes
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        let bytes: &[u8; 16] = bytes.try_into().ok()?;
        Some(Self([
            f64::from_le_bytes(bytes[..8].try_into().unwrap()),
            f64::from_le_bytes(bytes[8..].try_into().unwrap()),
        ]))
    }

    fn upgrade(from: u8, bytes: &[u8]) -> Option<Vec<u8>> {
        (from == 0).then(|| bytes.to_vec())
    }
}

fn parse(bytes: &[u8]) -> Option<[f64; 2]> {
    match decode_versioned_or_legacy::<Stored>(bytes, LEGACY_LEN) {
        Ok(stored) => Some(stored.0),
        Err(error) => {
            log(&format!(
                "weather: {KV_OFF} is unreadable ({error}); the deck starts at rest"
            ));
            None
        }
    }
}

pub fn load() -> [f64; 2] {
    world_kv_get(KV_OFF)
        .and_then(|bytes| parse(&bytes))
        .unwrap_or([0.0, 0.0])
}

#[derive(Default)]
pub struct Advection {
    pub off: [f64; 2],
    last_clock: Option<u64>,
}

impl Advection {
    pub fn new(off: [f64; 2]) -> Self {
        Self {
            off,
            last_clock: None,
        }
    }

    pub fn step(&mut self, clock: u64, seed: u32) -> bool {
        let moved = self.last_clock.is_some_and(|last| last != clock);
        if moved {
            self.off = advance_offset(self.off, clock, seed);
        }
        self.last_clock = Some(clock);
        moved
    }

    pub fn store(&self) {
        world_kv_store(KV_OFF, &Stored(self.off));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stored_offset_round_trips_versioned() {
        let off = [123.25, -0.5];
        let bytes = encode_versioned(&Stored(off));
        assert_eq!(bytes.len(), 17);
        assert_eq!(bytes[0], Stored::VERSION);
        assert_eq!(parse(&bytes), Some(off));
    }

    #[test]
    fn an_unversioned_offset_from_an_earlier_build_still_reads() {
        let mut legacy = 7.5f64.to_le_bytes().to_vec();
        legacy.extend(3.0f64.to_le_bytes());
        assert_eq!(parse(&legacy), Some([7.5, 3.0]));
    }

    #[test]
    fn the_first_tick_only_latches_the_clock() {
        let mut advection = Advection::new([4.0, 2.0]);
        assert!(!advection.step(1_000, 77));
        assert_eq!(advection.off, [4.0, 2.0]);
    }

    #[test]
    fn a_frozen_clock_freezes_the_deck() {
        let mut advection = Advection::new([4.0, 2.0]);
        advection.step(1_000, 77);
        assert!(!advection.step(1_000, 77));
        assert_eq!(advection.off, [4.0, 2.0]);
    }

    #[test]
    fn a_moving_clock_advances_exactly_as_the_shared_field_does() {
        let mut advection = Advection::new([4.0, 2.0]);
        advection.step(1_000, 77);
        assert!(advection.step(1_001, 77));
        assert_eq!(advection.off, advance_offset([4.0, 2.0], 1_001, 77));
    }
}
