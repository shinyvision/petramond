//! The graph events client mods fired locally that the server is still due
//! to echo. A mod running its rule on both sides fires once here and once
//! on the authority; the echo of the authority's fire must not play the
//! event twice, so each local fire absorbs ONE matching echo. A fire
//! nobody echoes expires after a bounded span of time, because a permanent
//! per-key latch would swallow every later server-only fire of the same
//! event for the rest of the session.

use crate::player::RigId;

/// How long a local fire waits for its echo, in seconds: comfortably past a
/// round trip, and short enough that a fire the server never mirrors stops
/// masking its later fires. Time, not frames or ticks — the drain runs once
/// per frame, and a window counted in frames shrank below a round trip at
/// high refresh rates.
const ECHO_WINDOW: f64 = 1.0;

#[derive(Debug, Default)]
pub(super) struct PendingFires {
    /// `(rig, event, clock when fired)`, oldest first.
    entries: Vec<(RigId, u16, f64)>,
    /// Seconds advanced so far.
    clock: f64,
}

impl PendingFires {
    /// `dt` seconds passed: fires older than the window expire.
    pub fn advance(&mut self, dt: f32) {
        self.clock += f64::from(dt.max(0.0));
        let cutoff = self.clock - ECHO_WINDOW;
        self.entries.retain(|&(_, _, fired)| fired > cutoff);
    }

    /// A client mod fired `(rig, event)` just now.
    pub fn fired(&mut self, rig: RigId, event: u16) {
        self.entries.push((rig, event, self.clock));
    }

    /// The server echoed `(rig, event)`: drop the oldest local fire waiting
    /// for it and answer `true`; `false` means nothing local claims it.
    pub fn absorb(&mut self, rig: RigId, event: u16) -> bool {
        match self
            .entries
            .iter()
            .position(|&(r, e, _)| (r, e) == (rig, event))
        {
            Some(at) => {
                self.entries.remove(at);
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A local fire absorbs exactly one echo, and waits for it the same span
    /// of TIME at any frame rate: a window counted in frames expired before
    /// a round trip at high refresh, so the echo played the event twice.
    #[test]
    fn a_local_fire_absorbs_one_echo_for_the_same_time_at_any_frame_rate() {
        let (rig, swing, other) = (RigId(0), 3, 4);
        let mut pending = PendingFires::default();
        pending.fired(rig, swing);
        pending.fired(rig, swing);
        assert!(
            !pending.absorb(rig, other),
            "an event nobody fired locally is the server's"
        );
        assert!(pending.absorb(rig, swing));
        assert!(pending.absorb(rig, swing), "two fires, two echoes");
        assert!(
            !pending.absorb(rig, swing),
            "the third echo is a new server fire"
        );

        for frame_dt in [1.0 / 30.0, 1.0 / 240.0] {
            let mut pending = PendingFires::default();
            pending.fired(rig, swing);
            for _ in 0..(0.9 / frame_dt) as usize {
                pending.advance(frame_dt);
            }
            assert!(
                pending.absorb(rig, swing),
                "still waiting 0.9 s later at {frame_dt}"
            );
            pending.fired(rig, other);
            for _ in 0..(1.1 / frame_dt) as usize {
                pending.advance(frame_dt);
            }
            assert!(
                !pending.absorb(rig, other),
                "expired past the window at {frame_dt}"
            );
        }
    }
}
