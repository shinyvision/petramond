//! The pacing of dig feedback — dust off the struck face and the block's dig
//! hit — shared by the local player's mining and every replicated digger.

/// Seconds between dust flecks.
const DUST_INTERVAL: f32 = 0.1;
/// Seconds between dig hits: the player's own mining repeat.
const HIT_INTERVAL: f32 = 0.3;

#[derive(Clone, Copy, Default)]
pub(super) struct DigFeedback {
    dust: f32,
    hit: f32,
}

/// What is due after an advance.
#[derive(Clone, Copy, Default)]
pub(super) struct DigPulse {
    pub dust: bool,
    pub hit: bool,
}

impl DigFeedback {
    /// A dig whose first dust and hit land at once, not a beat later.
    pub(super) fn primed() -> Self {
        Self {
            dust: DUST_INTERVAL,
            hit: HIT_INTERVAL,
        }
    }

    pub(super) fn advance(&mut self, dt: f32) -> DigPulse {
        let due = |banked: &mut f32, interval: f32| {
            // One long frame is one pulse, never a burst of them.
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
