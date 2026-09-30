#[derive(Clone, Debug, PartialEq)]
pub struct Draw {
    pub full_ticks: u32,
    pub strain_ticks: u32,
    pub speed_scale: f32,
    pub launch_speed: [f32; 2],
}

impl Draw {
    pub fn launch_speed(&self, ticks: u32) -> f32 {
        let full = self.full_ticks.max(1);
        let t = if full == 1 {
            1.0
        } else {
            (ticks.clamp(1, full) - 1) as f32 / (full - 1) as f32
        };
        let [weak, strong] = self.launch_speed;
        weak + (strong - weak) * t
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Press<'a> {
    Held(&'a Draw),
    Released,
    Lost,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum State {
    Idle,
    Drawing(f32),
    Spent,
}

#[derive(Default, Clone, Debug, PartialEq)]
pub struct Clock {
    held: f32,
    drawing: bool,
    spent: bool,
    full: u32,
}

impl Clock {
    pub fn state(&self) -> State {
        match (self.drawing, self.spent) {
            (false, _) => State::Idle,
            (true, true) => State::Spent,
            (true, false) => State::Drawing(self.held),
        }
    }

    pub fn step(&mut self, press: Press, dt_ticks: f32) -> Option<u32> {
        let mut loosed = None;
        match press {
            Press::Held(draw) => {
                self.drawing = true;
                self.full = draw.full_ticks;
                self.held += dt_ticks.max(0.0);
                if !self.spent && self.held >= (draw.full_ticks + draw.strain_ticks) as f32 {
                    self.spent = true;
                    loosed = Some(draw.full_ticks);
                }
            }
            Press::Released => {
                if self.drawing && !self.spent {
                    let ticks = self.held.floor().min(self.full as f32) as u32;
                    loosed = (ticks >= 1).then_some(ticks);
                }
                *self = Clock::default();
            }
            Press::Lost => *self = Clock::default(),
        }
        loosed
    }
}
