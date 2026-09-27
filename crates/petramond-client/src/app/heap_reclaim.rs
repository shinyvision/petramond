use std::time::{Duration, Instant};

extern crate mimalloc as _;

const QUIET_FOR: Duration = Duration::from_secs(2);

#[derive(Default)]
pub(super) struct IdleHeapReclaim {
    quiet_since: Option<Instant>,
    armed: bool,
}

impl IdleHeapReclaim {
    pub(super) fn frame(&mut self, busy: bool, now: Instant) {
        if self.should_reclaim(busy, now) {
            reclaim();
        }
    }

    fn should_reclaim(&mut self, busy: bool, now: Instant) -> bool {
        if busy {
            self.quiet_since = None;
            self.armed = true;
            return false;
        }
        if !self.armed {
            return false;
        }
        let since = *self.quiet_since.get_or_insert(now);
        if now.duration_since(since) < QUIET_FOR {
            return false;
        }
        self.quiet_since = None;
        self.armed = false;
        true
    }
}

fn reclaim() {
    unsafe { mi_collect(true) };
}

extern "C" {
    fn mi_collect(force: bool);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fires_once_after_the_quiet_period_and_rearms_on_busy() {
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut r = IdleHeapReclaim::default();
        assert!(!r.should_reclaim(false, at(0)));
        assert!(!r.should_reclaim(false, at(5_000)));

        assert!(!r.should_reclaim(true, at(5_000)));
        assert!(!r.should_reclaim(false, at(5_100)), "quiet stretch starts");
        assert!(!r.should_reclaim(false, at(7_000)), "1.9 s quiet");
        assert!(r.should_reclaim(false, at(7_100)), "2 s quiet");
        assert!(
            !r.should_reclaim(false, at(20_000)),
            "one collect per cycle"
        );

        assert!(!r.should_reclaim(true, at(21_000)));
        assert!(!r.should_reclaim(false, at(21_500)));
        assert!(!r.should_reclaim(true, at(23_000)));
        assert!(!r.should_reclaim(false, at(23_100)));
        assert!(!r.should_reclaim(false, at(25_000)));
        assert!(r.should_reclaim(false, at(25_100)));
    }

    #[test]
    fn the_delay_is_wall_time_not_frame_count() {
        let t0 = Instant::now();
        let mut r = IdleHeapReclaim::default();
        r.should_reclaim(true, t0);
        assert!(!r.should_reclaim(false, t0));
        assert!(r.should_reclaim(false, t0 + QUIET_FOR));
    }
}
