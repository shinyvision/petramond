use crate::events::tick::TICK_DT;

use super::MAX_TICKS_PER_FRAME;

const AUTOSAVE_SECS: f32 = 30.0;

pub struct FrameClock {
    accumulator: f32,
    paused: bool,
    lan_ever_opened: bool,
    autosave_t: f32,
}

impl FrameClock {
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

    #[cfg(test)]
    pub fn lan_ever_opened(&self) -> bool {
        self.lan_ever_opened
    }

    pub fn request_pause(&mut self, paused: bool) {
        if !self.lan_ever_opened {
            self.paused = paused;
        }
    }

    pub fn open_to_lan(&mut self) {
        self.lan_ever_opened = true;
        self.paused = false;
    }

    pub fn hold(&mut self) {
        self.accumulator = 0.0;
    }

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

    pub fn until_next_tick(&self) -> f32 {
        (TICK_DT - self.accumulator).max(0.0)
    }

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
        assert_eq!(
            clock.due_ticks(10.0),
            MAX_TICKS_PER_FRAME,
            "a stall is capped"
        );
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
