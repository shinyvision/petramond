//! An inaudible, never-ending keep-alive signal mixed into the output for the whole
//! session, so the OS audio device — and especially a **Bluetooth** link — never
//! goes idle and pays a 200-500 ms cold-start, which would delay the first sound
//! after a silent gap.
//!
//! This is the standard technique behind audio keep-awake tools (e.g. Sound Keeper)
//! and what interactive audio relies on: hold ONE output stream open for the session
//! AND keep a tiny non-zero signal flowing, so the device never detects "silence" and
//! suspends. Pure digital zeros do not work — drivers and BT stacks treat an
//! all-zero stream as idle and sleep anyway (which is why merely keeping the stream
//! open isn't enough). So we emit ultra-low-amplitude white noise: broadband (no
//! audible tone), reliably non-zero through the float → codec pipeline, and so quiet
//! (~-84 dBFS) it is far below audibility at any sane playback level.

use std::time::Duration;

use rodio::source::Source;
use rodio::{ChannelCount, Sample, SampleRate};

/// Peak amplitude of the keep-alive noise (linear; full scale = 1.0). ~-84 dBFS —
/// about two 16-bit LSBs: inaudible, yet non-zero after quantisation so the device
/// treats the stream as active. The one tuning knob if some device still sleeps
/// (raise it) or a hiss is ever audible (lower it).
const AMPLITUDE: f32 = 1.0 / 16_384.0;

/// An endless, inaudible white-noise [`Source`] that keeps the audio device awake.
/// Add one to the output mixer once at startup; it plays (silently) for the session
/// and is dropped with the stream at exit.
pub(super) struct KeepAlive {
    channels: ChannelCount,
    sample_rate: SampleRate,
    /// xorshift64 state. Presentation-only; the exact sequence is irrelevant (it's
    /// noise), so a fixed non-zero seed is fine.
    rng: u64,
}

impl KeepAlive {
    pub(super) fn new(channels: ChannelCount, sample_rate: SampleRate) -> Self {
        Self {
            channels,
            sample_rate,
            rng: 0x9E37_79B9_7F4A_7C15,
        }
    }
}

impl Iterator for KeepAlive {
    type Item = Sample;

    #[inline]
    fn next(&mut self) -> Option<Sample> {
        // xorshift64 → a noise sample in [-AMPLITUDE, AMPLITUDE). Never `None` — this
        // source is endless, which is what keeps the device perpetually awake.
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        let unit = (x >> 40) as f32 / (1u32 << 24) as f32; // [0, 1)
        Some((unit * 2.0 - 1.0) * AMPLITUDE)
    }
}

impl Source for KeepAlive {
    #[inline]
    fn current_span_len(&self) -> Option<usize> {
        None // uniform stream, unbounded
    }

    #[inline]
    fn channels(&self) -> ChannelCount {
        self.channels
    }

    #[inline]
    fn sample_rate(&self) -> SampleRate {
        self.sample_rate
    }

    #[inline]
    fn total_duration(&self) -> Option<Duration> {
        None // never ends
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keep_alive_is_endless_bounded_and_nonzero() {
        let mut k = KeepAlive::new(
            ChannelCount::new(2).unwrap(),
            SampleRate::new(48_000).unwrap(),
        );
        let mut any_nonzero = false;
        for _ in 0..10_000 {
            let s = k.next().expect("keep-alive never ends");
            assert!(s.abs() <= AMPLITUDE, "stays inaudible (<= {AMPLITUDE})");
            any_nonzero |= s != 0.0;
        }
        // The whole point: a non-zero signal, so the device never sees silence.
        assert!(
            any_nonzero,
            "must emit non-zero samples to keep the device awake"
        );
    }
}
