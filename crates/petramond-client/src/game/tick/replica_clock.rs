//! The render-time clock over received tick batches.

use super::TICK_DT;

/// Client-side PHASE-ACCUMULATOR clock over a STAGED interpolation window.
/// `tick_alpha` used to read the server accumulator, which now lives on the
/// server thread; tying it to each update's arrival time instead makes batch
/// arrivals quantized to frame boundaries
/// and jittered by thread scheduling; the old timestamp-reset clock aliased
/// that into a stall/lurch cycle — every early batch shifted the pair under a
/// mid-segment render (a forward jump), every late one pinned alpha at 1 (a
/// stall). Invisible on a walking sheep; violent rubber-banding with the
/// camera glued to a 4.5 m/s boat (the 2026-07-15 riding bug).
///
/// Now fresh batches enter a small bounded FIFO (`Game::staged_rows`): render
/// time advances by the frame's real `dt` ([`advance`](Self::advance)) and
/// queued rows COMMIT only when the phase crosses segment boundaries
/// (`Game::advance_interp_window` → [`consume_segment`](Self::consume_segment)),
/// so the pair under the render never shifts mid-segment and steady batches
/// render at constant velocity no matter how arrivals alias against the frame
/// rate. The timeline self-centres by a one-sided ratchet: only a genuinely
/// late batch slips it ([`hold`](Self::hold) drops the starved excess). If the
/// queue overflows, pending snapshots collapse to the newest state (all player
/// actions retained in order) and snap prev == curr at the NEXT crossed
/// boundary; even emergency catch-up never mutates a live segment.
#[derive(Default)]
pub struct ReplicaClock {
    /// Render-time phase in fixed ticks past the committed pair: `advance`
    /// adds real time, `consume_segment` subtracts a committed batch,
    /// [`alpha`](Self::alpha) clamps into the pair's interpolation range.
    phase: f32,
    started: bool,
}

impl ReplicaClock {
    /// Advance render time by one frame of real `dt` (called once per frame,
    /// before anything samples [`alpha`](Self::alpha)).
    pub fn advance(&mut self, dt: f32) {
        if self.started {
            self.phase += dt / TICK_DT;
        }
    }

    /// The first batch bootstrapped the stores (prev == curr): start the
    /// render timeline at the segment origin.
    pub fn start(&mut self) {
        self.started = true;
        self.phase = 0.0;
    }

    pub fn started(&self) -> bool {
        self.started
    }

    /// Render time has crossed the current segment — the next staged batch
    /// (if any) is due to commit.
    pub fn overdue(&self) -> bool {
        self.started && self.phase >= 1.0
    }

    /// A staged batch committed: the render window moved one segment forward.
    pub fn consume_segment(&mut self) {
        self.phase = (self.phase - 1.0).max(0.0);
    }

    /// Nothing staged while overdue (a late batch, a pause): hold at the
    /// segment end instead of extrapolating, dropping the starved excess —
    /// the ratchet that keeps the timeline just far enough behind arrivals.
    pub fn hold(&mut self) {
        self.phase = self.phase.min(1.0);
    }

    /// Fraction (0..1) into the committed prev→curr pair. `1.0` before the
    /// first update (render current state, no interpolation).
    pub fn alpha(&self) -> f32 {
        if self.started {
            self.phase.clamp(0.0, 1.0)
        } else {
            1.0
        }
    }
}
