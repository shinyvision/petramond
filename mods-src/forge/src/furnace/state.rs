//! One furnace's PERSISTED state: the blob, its layout version, and the two
//! questions the blob alone can answer.
//!
//! Split from the machine that drives it because it changes for its own
//! reason — a field added, removed or reordered — and that change has its own
//! discipline (bump [`STATE_VERSION`], extend BOTH halves) which has nothing
//! to do with how the furnace melts or pours.

use mod_sdk::*;

use crate::liquid::Liquid;

/// How long an unlit furnace's crucible stays pourable.
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

/// One furnace's whole state, in section cell KV at the anchor, so a furnace
/// mid-pour reloads mid-pour and a crucible left to harden is still hard.
#[derive(Clone, Default, PartialEq)]
pub(super) struct State {
    pub(super) burn_remaining: u32,
    pub(super) burn_max: u32,
    pub(super) melt_progress: u32,
    /// Ticks the fire has been out. Past `HARDEN_TICKS` the crucible has set.
    pub(super) idle_ticks: u32,
    pub(super) units: u8,
    pub(super) phase: Phase,
    pub(super) phase_ticks: u32,
    /// The item the crucible's metal was melted FROM — the ingredient half of
    /// the cast lookup, and empty when the crucible is.
    pub(super) metal: String,
    /// The molten metal's continuous state — the part of a pour that is a
    /// number rather than a stage.
    pub(super) liquid: Liquid,
}

/// Bumped whenever a field is added, removed or reordered. The blob is
/// POSITIONAL, so a mismatched layout does not fail to parse — it shifts every
/// field after the change and hands the machine someone else's numbers, which
/// surfaces as a furnace full of metal it never melted or a pour that never
/// ends. An unrecognised version resets the machine instead.
const STATE_VERSION: u32 = 3;

impl State {
    pub(super) fn decode(bytes: &[u8]) -> State {
        let mut r = ByteReader::new(bytes);
        if bytes.is_empty() {
            return State::default();
        }
        if r.u32() != Some(STATE_VERSION) {
            return State::default();
        }
        State {
            burn_remaining: r.u32().unwrap_or(0),
            burn_max: r.u32().unwrap_or(0),
            melt_progress: r.u32().unwrap_or(0),
            idle_ticks: r.u32().unwrap_or(0),
            units: r.u32().unwrap_or(0) as u8,
            phase: Phase::from_u8(r.u32().unwrap_or(0) as u8),
            phase_ticks: r.u32().unwrap_or(0),
            metal: read_str(&mut r),
            liquid: Liquid::decode(&mut r),
        }
    }

    pub(super) fn encode(&self) -> Vec<u8> {
        let mut w = ByteWriter::with_capacity(52);
        w.u32(STATE_VERSION);
        w.u32(self.burn_remaining);
        w.u32(self.burn_max);
        w.u32(self.melt_progress);
        w.u32(self.idle_ticks);
        w.u32(self.units as u32);
        w.u32(self.phase.to_u8() as u32);
        w.u32(self.phase_ticks);
        write_str(&mut w, &self.metal);
        self.liquid.encode(&mut w);
        w.finish()
    }

    /// The state-only half of "may the lever be pulled": metal in the
    /// crucible, still liquid, nothing already running. The other half is
    /// whether the pour has anywhere to GO — see
    /// [`ForgingFurnaceSpec::pourable`](super::ForgingFurnaceSpec::pourable).
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

    /// The furnace's whole machine state is one cell-KV blob, decoded
    /// positionally: adding a field without extending BOTH halves silently
    /// shifts every field after it, which reads as a furnace that reloads with
    /// someone else's metal in it.
    #[test]
    fn state_survives_the_kv_round_trip() {
        let mut state = State {
            burn_remaining: 733,
            burn_max: 1600,
            melt_progress: 41,
            idle_ticks: 9,
            units: 6,
            phase: Phase::Setting,
            phase_ticks: 77,
            metal: "forge:raw_copper".into(),
            liquid: Liquid::default(),
        };
        state.liquid.step(true, 0.05);

        let back = State::decode(&state.encode());
        assert!(back == state, "every field, not just the ones a test names");
    }

    /// An empty blob is what a freshly placed furnace reads, so the decode has
    /// to answer with a cold, empty machine rather than a partly-filled one.
    #[test]
    fn an_unwritten_furnace_decodes_cold() {
        let fresh = State::decode(&[]);
        assert!(fresh == State::default());
    }

    /// A blob written by an older layout must reset the machine, not be read
    /// as if it were the current one.
    #[test]
    fn a_foreign_state_blob_resets_rather_than_misreads() {
        let mut w = ByteWriter::new();
        w.u32(STATE_VERSION.wrapping_sub(1));
        w.u32(999);
        assert!(
            State::decode(&w.finish()) == State::default(),
            "an unrecognised version is a reset"
        );
    }
}
