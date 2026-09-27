//! The presentation clock: the wall, or a stepped timeline a mod holds.
//!
//! Stepped, time is COUNTED, not measured: frame `n` is at `start + n·step`
//! exactly, whatever the wall clock did meanwhile. A frame moves time only
//! when an advance was queued before it (a capture taken with `advance`, or
//! the holder's own call); every other frame is HELD, with no time passing.
//! N advances therefore always span exactly N steps.

/// A mod's stepped timeline.
#[derive(Clone, Debug, PartialEq)]
pub struct SteppedClock {
    start: f64,
    step: f64,
    /// Steps taken so far.
    steps: u64,
    /// Advances queued for the next frame.
    pending: u64,
}

impl SteppedClock {
    pub fn new(start: f64, step: f64) -> Self {
        Self {
            start,
            step,
            steps: 0,
            pending: 0,
        }
    }

    pub fn step_seconds(&self) -> f64 {
        self.step
    }

    pub fn now(&self) -> f64 {
        self.start + self.steps as f64 * self.step
    }

    /// Queue `n` advances: the next frame moves on by that many steps.
    pub fn advance(&mut self, n: u64) {
        self.pending += n;
    }

    /// Advances queued for the next frame.
    pub fn pending(&self) -> u64 {
        self.pending
    }

    /// Start the next frame: the time it moves on by, the queued steps, or
    /// nothing for a held frame.
    pub fn take_step(&mut self) -> f64 {
        let n = std::mem::take(&mut self.pending);
        self.steps += n;
        n as f64 * self.step
    }
}

/// What every presentation system reads as "now". Never runs backwards: a
/// stepped timeline starts where the wall was, and the wall resumes from
/// where the timeline stopped.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PresentationClock {
    /// Wall seconds the presentation runs behind (or ahead of) the wall.
    offset: f64,
    stepped: Option<SteppedClock>,
}

impl PresentationClock {
    pub fn now(&self, wall: f64) -> f64 {
        match &self.stepped {
            Some(clock) => clock.now(),
            None => wall - self.offset,
        }
    }

    pub fn stepped(&self) -> Option<&SteppedClock> {
        self.stepped.as_ref()
    }

    pub fn stepped_mut(&mut self) -> Option<&mut SteppedClock> {
        self.stepped.as_mut()
    }

    /// Step by `step` seconds from now on (a new step keeps the time where
    /// it is, and drops advances queued at the old one).
    pub fn begin_stepped(&mut self, wall: f64, step: f64) {
        let start = self.now(wall);
        self.stepped = Some(SteppedClock::new(start, step));
    }

    pub fn end_stepped(&mut self, wall: f64) {
        if let Some(clock) = self.stepped.take() {
            self.offset = wall - clock.now();
        }
    }
}
