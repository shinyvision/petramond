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

/// Consecutive quiet frames before a reclaim fires. At 60 fps this is ~2 s of
/// settled terrain: long enough that a burst never pays for it, short enough
/// that standing still after a flight gives the memory back promptly.
const QUIET_FRAMES: u32 = 120;

/// Tracks terrain quiet and fires at most one reclaim per busy→quiet cycle.
#[derive(Default)]
pub(crate) struct IdleHeapReclaim {
    quiet: u32,
    /// Cleared by a reclaim, set again by any busy frame — so a settled world
    /// collects once, not every `QUIET_FRAMES`.
    armed: bool,
}

impl IdleHeapReclaim {
    /// Call once per rendered frame. `busy` is true while terrain is still
    /// streaming, meshing or uploading.
    pub(crate) fn frame(&mut self, busy: bool) {
        if busy {
            self.quiet = 0;
            self.armed = true;
            return;
        }
        if !self.armed {
            return;
        }
        self.quiet += 1;
        if self.quiet < QUIET_FRAMES {
            return;
        }
        self.quiet = 0;
        self.armed = false;
        reclaim();
    }
}

/// Return the allocator's free pages to the OS. Main thread only (see the
/// module doc); ~30 ms after a render-distance-32 stream.
pub(crate) fn reclaim() {
    // SAFETY: `mi_collect` is the mimalloc C entry point linked in by the
    // global allocator; it takes no pointers and is safe to call at any time.
    unsafe { mi_collect(true) };
}

extern "C" {
    fn mi_collect(force: bool);
}
