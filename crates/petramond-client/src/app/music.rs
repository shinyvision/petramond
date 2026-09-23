//! The music director: WHEN a soundtrack piece plays.
//!
//! The same split the footstep and mob-idle cadences use — presentation owns
//! the schedule, the audio engine owns the playback. [`Audio`] knows how to
//! stream one track and how loud it should be; everything about the rhythm of
//! a session's music — the quiet gap, which piece comes next, never the same
//! piece twice running — lives here.
//!
//! The clock is the WALL clock, not the game tick: music is not part of the
//! world. It keeps its place behind a pause menu (where the world's own sounds
//! freeze), and a track already playing plays on.

use petramond_audio::{Audio, MusicTrack};

/// The quiet gap before the FIRST track of a session. Shorter than the gaps
/// that follow, so a new world is scored fairly soon after you land in it
/// rather than after a full silence.
const FIRST_GAP: (f64, f64) = (25.0, 75.0);
/// The quiet gap between tracks. Music is a punctuation mark on a long session,
/// not a radio station: the silence between pieces is most of the experience,
/// and the world's own sounds are what carries it.
const GAP: (f64, f64) = (180.0, 420.0);

/// Per-session music scheduling state.
pub struct MusicDirector {
    /// Wall-clock time the next track is due. `None` = not scheduled yet (no
    /// session, or the session just started and the first gap is unset).
    next_at: Option<f64>,
    /// The piece that played last, never picked twice running while any other
    /// is available.
    last: Option<MusicTrack>,
    /// xorshift64 state for track choice and gap length — presentation-only
    /// randomness, seeded from the wall clock, never the worldgen RNG.
    rng: u64,
}

impl MusicDirector {
    pub fn new() -> Self {
        Self {
            next_at: None,
            last: None,
            rng: seed(),
        }
    }

    /// Drive the music channel for this frame. `in_session` is whether a world
    /// is loaded at all; `now` is the wall clock and `dt` the frame's seconds.
    ///
    /// Call it EVERY frame regardless of the open screen: a pause menu, an
    /// inventory or a container is still the same session, and music that
    /// stopped because you opened a chest would be a bug.
    pub fn update(&mut self, audio: &mut Audio, in_session: bool, now: f64, dt: f32) {
        if !in_session {
            // Leaving a world takes its music with it — faded, not cut — and
            // the next session starts its own schedule from silence.
            audio.stop_music();
            self.next_at = None;
            self.last = None;
        } else if audio.music_playing().is_some() {
            // A playing track owns the channel; the next gap is measured from
            // the moment it ENDS, so it is scheduled once it does.
            self.next_at = None;
        } else {
            match self.next_at {
                None => self.next_at = Some(now + self.gap(self.last.is_none())),
                Some(due) if now >= due => {
                    if let Some(track) = self.pick() {
                        if audio.play_music(track) {
                            self.last = Some(track);
                        }
                    }
                    // Re-rolled whether or not the track started: a missing or
                    // broken file must not stall the channel forever, and a
                    // track that DID start replaces this the next frame.
                    self.next_at = Some(now + self.gap(false));
                }
                Some(_) => {}
            }
        }
        audio.update_music(dt);
    }

    /// A random gap, from the opening range on a fresh session.
    fn gap(&mut self, first: bool) -> f64 {
        let (lo, hi) = if first { FIRST_GAP } else { GAP };
        lo + self.next_unit() * (hi - lo)
    }

    /// A random track other than the one that just played. With a single track
    /// loaded, that one repeats — the alternative is silence.
    fn pick(&mut self) -> Option<MusicTrack> {
        let count = petramond_world::music_registry::defs().len();
        if count == 0 {
            return None;
        }
        if count == 1 {
            return Some(MusicTrack(0));
        }
        // Pick among the OTHERS by drawing from `count - 1` and stepping over
        // the last played, which is uniform over them and cannot loop.
        let pick = (self.next_rng() % (count as u64 - 1)) as u8;
        let track = match self.last {
            Some(last) if pick >= last.0 => MusicTrack(pick + 1),
            _ => MusicTrack(pick),
        };
        Some(track)
    }

    #[inline]
    fn next_rng(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        x
    }

    /// Next value in `[0, 1)`.
    fn next_unit(&mut self) -> f64 {
        (self.next_rng() >> 11) as f64 / (1u64 << 53) as f64
    }
}

impl Default for MusicDirector {
    fn default() -> Self {
        Self::new()
    }
}

fn seed() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
        | 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn director(seed: u64) -> MusicDirector {
        MusicDirector {
            next_at: None,
            last: None,
            rng: seed,
        }
    }

    /// The pick must never repeat the last piece and must still be able to
    /// reach every OTHER one — the "step over the last" index arithmetic is
    /// exactly the kind of off-by-one that silently drops a track from the
    /// rotation (or, worse, plays one twice running) without failing anything.
    #[test]
    fn the_next_track_is_never_the_last_and_every_other_is_reachable() {
        let count = petramond_world::music_registry::defs().len();
        assert!(
            count >= 2,
            "the rotation needs alternatives to choose among"
        );
        let mut d = director(0xC0FF_EE12_3456_789B);
        for last in 0..count as u8 {
            d.last = Some(MusicTrack(last));
            let mut seen = vec![false; count];
            for _ in 0..1_000 {
                let picked = d.pick().expect("a track is available").0;
                assert!((picked as usize) < count, "track {picked} is out of range");
                assert_ne!(picked, last, "the last track played again immediately");
                seen[picked as usize] = true;
            }
            for (i, hit) in seen.iter().enumerate() {
                assert_eq!(
                    *hit,
                    i != last as usize,
                    "track {i} reachable after {last}: expected {}",
                    i != last as usize
                );
            }
        }
    }

    /// Gaps must land inside their declared range — an inverted or overflowing
    /// range would either spam tracks back to back or never play one at all.
    #[test]
    fn gaps_stay_inside_their_range() {
        let mut d = director(0x1234_5678_9ABC_DEF1);
        for _ in 0..1_000 {
            let first = d.gap(true);
            assert!(
                (FIRST_GAP.0..=FIRST_GAP.1).contains(&first),
                "first gap {first} out of range"
            );
            let gap = d.gap(false);
            assert!((GAP.0..=GAP.1).contains(&gap), "gap {gap} out of range");
        }
    }
}
