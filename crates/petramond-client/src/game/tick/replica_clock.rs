//! The render-time clock over received tick batches.

use super::TICK_DT;

/// Client-side PHASE-ACCUMULATOR clock over a STAGED interpolation window.
/// The server accumulator lives on the server thread. Batch arrivals are
/// quantized to frame boundaries and jittered by thread scheduling, so arrival
/// time cannot directly set `tick_alpha` without stalls and jumps.
///
/// Fresh batches enter a small bounded FIFO (`Game::staged_rows`): render
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
    phase: f32,
    started: bool,
}

impl ReplicaClock {
    pub fn advance(&mut self, dt: f32) {
        if self.started {
            self.phase += dt / TICK_DT;
        }
    }

    pub fn start(&mut self) {
        self.started = true;
        self.phase = 0.0;
    }

    pub fn started(&self) -> bool {
        self.started
    }

    pub fn overdue(&self) -> bool {
        self.started && self.phase >= 1.0
    }

    pub fn consume_segment(&mut self) {
        self.phase = (self.phase - 1.0).max(0.0);
    }

    pub fn hold(&mut self) {
        self.phase = self.phase.min(1.0);
    }

    pub fn place(&mut self, alpha: f32) {
        self.phase = alpha.clamp(0.0, 1.0);
    }

    pub fn alpha(&self) -> f32 {
        if self.started {
            self.phase.clamp(0.0, 1.0)
        } else {
            1.0
        }
    }
}
