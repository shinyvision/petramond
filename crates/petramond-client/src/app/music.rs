use petramond_audio::{Audio, MusicTrack};

const FIRST_GAP: (f64, f64) = (25.0, 75.0);
const GAP: (f64, f64) = (180.0, 420.0);

pub struct MusicDirector {
    remaining: Option<f64>,
    last: Option<MusicTrack>,
    rng: u64,
}

impl MusicDirector {
    pub fn new() -> Self {
        Self {
            remaining: None,
            last: None,
            rng: seed(),
        }
    }

    pub fn update(&mut self, audio: &mut Audio, in_session: bool, paused: bool, dt: f32) {
        if !in_session {
            audio.stop_music();
            self.remaining = None;
            self.last = None;
        } else if audio.music_playing().is_some() {
            self.remaining = None;
        } else if !paused {
            match self.remaining {
                None => self.remaining = Some(self.gap(self.last.is_none())),
                Some(left) => {
                    let left = left - dt.clamp(0.0, 1.0) as f64;
                    if left > 0.0 {
                        self.remaining = Some(left);
                    } else {
                        if let Some(track) = self.pick() {
                            if audio.play_music(track) {
                                self.last = Some(track);
                            }
                        }
                        self.remaining = Some(self.gap(false));
                    }
                }
            }
        }
        audio.update_music(dt);
    }

    fn gap(&mut self, first: bool) -> f64 {
        let (lo, hi) = if first { FIRST_GAP } else { GAP };
        lo + self.next_unit() * (hi - lo)
    }

    fn pick(&mut self) -> Option<MusicTrack> {
        let count = petramond_audio::music_registry::defs().len();
        if count == 0 {
            return None;
        }
        if count == 1 {
            return Some(MusicTrack(0));
        }
        let pick = (self.next_rng() % (count as u64 - 1)) as u8;
        let track = match self.last {
            Some(last) if pick >= last.0 => MusicTrack(pick + 1),
            _ => MusicTrack(pick),
        };
        Some(track)
    }

    #[cfg(test)]
    fn remaining(&self) -> Option<f64> {
        self.remaining
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
            remaining: None,
            last: None,
            rng: seed,
        }
    }

    #[test]
    fn the_next_track_is_never_the_last_and_every_other_is_reachable() {
        let count = petramond_audio::music_registry::defs().len();
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

    #[test]
    fn a_paused_game_holds_the_gap_and_starts_nothing() {
        let mut audio = Audio::new();
        let mut d = director(0x5EED_1234_ABCD_9876);

        d.update(&mut audio, true, false, 0.0);
        let rolled = d.remaining().expect("a gap is owed");
        d.update(&mut audio, true, false, 1.0);
        let ticked = d.remaining().expect("still owed");
        assert!(
            (rolled - ticked - 1.0).abs() < 1e-6,
            "an unpaused second should spend a second of the gap: {rolled} -> {ticked}"
        );

        for _ in 0..100 {
            d.update(&mut audio, true, true, 1.0);
        }
        assert_eq!(
            d.remaining(),
            Some(ticked),
            "the pause must not count toward the gap"
        );

        let mut fresh = director(0x1111_2222_3333_4445);
        for _ in 0..100 {
            fresh.update(&mut audio, true, true, 1.0);
        }
        assert_eq!(
            fresh.remaining(),
            None,
            "nothing may be scheduled while paused"
        );
    }

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
