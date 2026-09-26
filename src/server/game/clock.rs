//! The server's wall-clock scheduling: fixed-tick debt, the singleplayer
//! pause gate, and the autosave timer.

use crate::events::tick::TICK_DT;

use super::MAX_TICKS_PER_FRAME;

/// Seconds between background autosaves.
const AUTOSAVE_SECS: f32 = 30.0;

/// Wall-clock time banked toward fixed ticks and autosaves, plus the pause
/// gate. Scheduling bookkeeping, not sim state.
pub struct FrameClock {
    /// Wall-clock seconds banked toward the next fixed simulation tick.
    accumulator: f32,
    /// Singleplayer pause (`ClientToServer::Pause`): while set, the pump skips
    /// the fixed ticks ONLY — message drain, streaming, and autosave keep
    /// running — and banks no tick debt. Honored only while the server has
    /// never been open to LAN (the sole connection is the local one).
    paused: bool,
    /// Set (permanently, for the session) when "Open to LAN" first succeeds,
    /// or from boot on a headless server: remote players may exist (or
    /// reappear) at any time, so pause requests are ignored.
    lan_ever_opened: bool,
    /// Wall-clock seconds since the last background autosave.
    autosave_t: f32,
}

impl FrameClock {
    /// A fresh clock. `remote_from_boot` = a headless server, whose pause
    /// gate starts (and stays) open.
    pub fn new(remote_from_boot: bool) -> Self {
        Self {
            accumulator: 0.0,
            paused: false,
            lan_ever_opened: remote_from_boot,
            autosave_t: 0.0,
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Whether the server was ever open to remote players.
    pub fn lan_ever_opened(&self) -> bool {
        self.lan_ever_opened
    }

    /// A client's pause request: honored only while the server has never
    /// been open to LAN.
    pub fn request_pause(&mut self, paused: bool) {
        if !self.lan_ever_opened {
            self.paused = paused;
        }
    }

    /// "Open to LAN" succeeded: force-unpause and close the pause gate for
    /// good.
    pub fn open_to_lan(&mut self) {
        self.lan_ever_opened = true;
        self.paused = false;
    }

    /// The sim is frozen (paused, or nobody connected): bank no tick debt, so
    /// resuming never fast-forwards.
    pub fn hold(&mut self) {
        self.accumulator = 0.0;
    }

    /// Bank `dt` seconds (long stalls clamped to one second) and return how
    /// many fixed ticks are due now, capped at [`MAX_TICKS_PER_FRAME`] so the
    /// sim never spirals trying to replay lost time. Leftover debt beyond one
    /// tick is dropped.
    pub fn due_ticks(&mut self, dt: f32) -> u32 {
        self.accumulator += dt.clamp(0.0, 1.0);
        let mut due = 0;
        while self.accumulator >= TICK_DT && due < MAX_TICKS_PER_FRAME {
            self.accumulator -= TICK_DT;
            due += 1;
        }
        if self.accumulator > TICK_DT {
            self.accumulator = TICK_DT;
        }
        due
    }

    /// Seconds until the next fixed tick is due (the server loop's sleep
    /// bound).
    pub fn until_next_tick(&self) -> f32 {
        (TICK_DT - self.accumulator).max(0.0)
    }

    /// Advance the autosave timer by `dt`; `true` when a save is due (the
    /// timer restarts).
    pub fn autosave_due(&mut self, dt: f32) -> bool {
        self.autosave_t += dt;
        if self.autosave_t >= AUTOSAVE_SECS {
            self.autosave_t = 0.0;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_debt_is_capped_and_pause_honors_the_lan_gate() {
        let mut clock = FrameClock::new(false);
        assert_eq!(clock.due_ticks(TICK_DT * 2.5), 2);
        assert_eq!(clock.due_ticks(10.0), MAX_TICKS_PER_FRAME, "a stall is capped");
        assert!(clock.until_next_tick() >= 0.0);

        clock.request_pause(true);
        assert!(clock.is_paused());
        clock.open_to_lan();
        assert!(!clock.is_paused(), "opening to LAN force-unpauses");
        clock.request_pause(true);
        assert!(!clock.is_paused(), "and the gate stays closed");
        assert!(FrameClock::new(true).lan_ever_opened());
    }
}
