use machine_core::Burner;
use mod_sdk::*;

use crate::liquid::Liquid;

const HARDEN_TICKS: u32 = 200;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Phase {
    #[default]
    Idle,
    Pouring,
    Setting,
}

impl Phase {
    fn to_u8(self) -> u8 {
        match self {
            Phase::Idle => 0,
            Phase::Pouring => 1,
            Phase::Setting => 2,
        }
    }
    fn from_u8(v: u8) -> Phase {
        match v {
            1 => Phase::Pouring,
            2 => Phase::Setting,
            _ => Phase::Idle,
        }
    }
}

#[derive(Clone, Default, PartialEq)]
pub(super) struct State {
    pub(super) fire: Burner,
    pub(super) melt_progress: u32,
    pub(super) idle_ticks: u32,
    pub(super) units: u8,
    pub(super) phase: Phase,
    pub(super) phase_ticks: u32,
    pub(super) ready_ticks: u32,
    pub(super) feed_ticks: u32,
    pub(super) ready_mould: String,
    pub(super) pour_mould: String,
    pub(super) metal: String,
    pub(super) liquid: Liquid,
}

impl KvRecord for State {
    const VERSION: u8 = 5;
    const OLDEST_VERSION: u8 = 4;

    fn decode(bytes: &[u8]) -> Option<Self> {
        let mut r = ByteReader::new(bytes);
        Some(State {
            fire: Burner::decode(&mut r),
            melt_progress: r.u32().unwrap_or(0),
            idle_ticks: r.u32().unwrap_or(0),
            units: r.u32().unwrap_or(0) as u8,
            phase: Phase::from_u8(r.u32().unwrap_or(0) as u8),
            phase_ticks: r.u32().unwrap_or(0),
            ready_ticks: r.u32().unwrap_or(0),
            feed_ticks: r.u32().unwrap_or(0),
            ready_mould: read_str(&mut r),
            pour_mould: read_str(&mut r),
            metal: read_str(&mut r),
            liquid: Liquid::decode(&mut r),
        })
    }

    fn encode(&self) -> Vec<u8> {
        let mut w = ByteWriter::with_capacity(52);
        self.fire.encode(&mut w);
        w.u32(self.melt_progress);
        w.u32(self.idle_ticks);
        w.u32(self.units as u32);
        w.u32(self.phase.to_u8() as u32);
        w.u32(self.phase_ticks);
        w.u32(self.ready_ticks);
        w.u32(self.feed_ticks);
        write_str(&mut w, &self.ready_mould);
        write_str(&mut w, &self.pour_mould);
        write_str(&mut w, &self.metal);
        self.liquid.encode(&mut w);
        w.finish()
    }

    fn upgrade(from: u8, bytes: &[u8]) -> Option<Vec<u8>> {
        match (from, bytes) {
            (4, [0, 0, 0, rest @ ..]) => Some(rest.to_vec()),
            _ => None,
        }
    }
}

impl State {
    pub(super) fn load(bytes: &[u8]) -> State {
        if bytes.is_empty() {
            return State::default();
        }
        decode_versioned(bytes).unwrap_or_default()
    }

    pub(super) fn to_bytes(&self) -> Vec<u8> {
        encode_versioned(self)
    }

    pub(super) fn ready_to_pour(&self) -> bool {
        self.phase == Phase::Idle && self.units > 0 && !self.hardened()
    }

    pub(super) fn hardened(&self) -> bool {
        self.units > 0 && self.idle_ticks >= HARDEN_TICKS
    }
}

fn write_str(w: &mut ByteWriter, s: &str) {
    w.blob(s.as_bytes());
}

fn read_str(r: &mut ByteReader) -> String {
    r.blob()
        .and_then(|b| std::str::from_utf8(b).ok())
        .unwrap_or_default()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_survives_the_kv_round_trip() {
        let mut state = State {
            fire: Burner {
                remaining: 733,
                max: 1600,
            },
            melt_progress: 41,
            idle_ticks: 9,
            units: 6,
            phase: Phase::Setting,
            phase_ticks: 77,
            ready_ticks: 13,
            feed_ticks: 7,
            ready_mould: "test:mould".into(),
            pour_mould: "test:mould".into(),
            metal: "forge:raw_copper".into(),
            liquid: Liquid::default(),
        };
        state.liquid.step(true, 0.05);

        let back = State::load(&state.to_bytes());
        assert!(back == state, "every field, not just the ones a test names");
    }

    #[test]
    fn a_version_4_blob_migrates() {
        let state = State {
            units: 3,
            metal: "forge:raw_copper".into(),
            ..State::default()
        };
        let mut old = 4u32.to_le_bytes().to_vec();
        old.extend(KvRecord::encode(&state));
        assert!(State::load(&old) == state);
    }

    #[test]
    fn an_unwritten_furnace_decodes_cold() {
        let fresh = State::load(&[]);
        assert!(fresh == State::default());
    }

    #[test]
    fn a_foreign_state_blob_resets_rather_than_misreads() {
        let mut w = ByteWriter::new();
        w.u32(3);
        w.u32(999);
        assert!(
            State::load(&w.finish()) == State::default(),
            "an unrecognised version is a reset"
        );
    }
}
