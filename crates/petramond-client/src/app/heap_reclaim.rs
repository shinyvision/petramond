//! Idle heap reclaim.
//!
//! The gen/light/mesh pools allocate on worker threads and the results are
//! dropped on the main thread, so a streaming burst leaves the main thread's
//! mimalloc heap holding hundreds of megabytes of free pages that it never
//! returns to the OS on its own: measured at render distance 32, 1.88 GB
//! resident against 112 MB of live world data, of which one forced collect
//! returns ~580 MB in ~31 ms.
//!
//! Continuous purging (`MIMALLOC_PURGE_DELAY=25`) reclaims the same memory
//! automatically but costs ~6% of mesh worker CPU, and a collect from a
//! background thread reclaims NOTHING — the free pages belong to the main
//! thread's heap. So the reclaim runs on the main thread, once, only after
//! terrain has been quiet long enough that the frame it lands in is not one
//! the player is streaming through.
//!
//! It lives in the client because the client is what chooses mimalloc (its
//! binaries install it as the global allocator and this crate depends on it),
//! so the `mi_collect` symbol it calls is always linked in.

use std::time::{Duration, Instant};

// Links libmimalloc, which defines the `mi_collect` declared below, even into
// a build of this crate that installs no global allocator of its own.
extern crate mimalloc as _;

/// Settled-terrain time before a reclaim fires: long enough that a burst never
/// pays for it, short enough that standing still after a flight gives the
/// memory back promptly. Wall time, so the delay does not depend on the frame
/// rate.
const QUIET_FOR: Duration = Duration::from_secs(2);

/// Tracks terrain quiet and fires at most one reclaim per busy→quiet cycle.
#[derive(Default)]
pub(super) struct IdleHeapReclaim {
    /// When the current quiet stretch began; `None` while busy or disarmed.
    quiet_since: Option<Instant>,
    /// Cleared by a reclaim, set again by any busy frame — so a settled world
    /// collects once, not every `QUIET_FOR`.
    armed: bool,
}

impl IdleHeapReclaim {
    /// Call once per rendered frame. `busy` is true while terrain is still
    /// streaming, meshing or uploading.
    pub(super) fn frame(&mut self, busy: bool, now: Instant) {
        if self.should_reclaim(busy, now) {
            reclaim();
        }
    }

    /// The reclaim decision: whether this frame ends a quiet stretch of at
    /// least [`QUIET_FOR`] since the last busy frame.
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

/// Return the allocator's free pages to the OS. Main thread only (see the
/// module doc); ~30 ms after a render-distance-32 stream.
fn reclaim() {
    // SAFETY: `mi_collect` is the mimalloc C entry point (linked through the
    // `mimalloc` dependency above); it takes no pointers and is safe to call
    // at any time.
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
        // Never busy: nothing to give back.
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

        // A busy frame mid-stretch restarts the clock.
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
        // Two frames far apart (a slow machine) are enough.
        assert!(!r.should_reclaim(false, t0));
        assert!(r.should_reclaim(false, t0 + QUIET_FOR));
    }
}
