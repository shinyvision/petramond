#[derive(Clone, Debug, PartialEq)]
pub struct SteppedClock {
    start: f64,
    step: f64,
    steps: u64,
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

    pub fn advance(&mut self, n: u64) {
        self.pending += n;
    }

    pub fn pending(&self) -> u64 {
        self.pending
    }

    pub fn take_step(&mut self) -> f64 {
        let n = std::mem::take(&mut self.pending);
        self.steps += n;
        n as f64 * self.step
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PresentationClock {
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
