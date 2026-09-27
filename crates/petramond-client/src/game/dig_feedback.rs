const DUST_INTERVAL: f32 = 0.1;
const HIT_INTERVAL: f32 = 0.3;

#[derive(Clone, Copy, Default)]
pub(super) struct DigFeedback {
    dust: f32,
    hit: f32,
}

#[derive(Clone, Copy, Default)]
pub(super) struct DigPulse {
    pub dust: bool,
    pub hit: bool,
}

impl DigFeedback {
    pub(super) fn primed() -> Self {
        Self {
            dust: DUST_INTERVAL,
            hit: HIT_INTERVAL,
        }
    }

    pub(super) fn advance(&mut self, dt: f32) -> DigPulse {
        let due = |banked: &mut f32, interval: f32| {
            *banked += dt.clamp(0.0, interval);
            let due = *banked >= interval;
            if due {
                *banked = 0.0;
            }
            due
        };
        DigPulse {
            dust: due(&mut self.dust, DUST_INTERVAL),
            hit: due(&mut self.hit, HIT_INTERVAL),
        }
    }
}

#[cfg(test)]
mod tests;
