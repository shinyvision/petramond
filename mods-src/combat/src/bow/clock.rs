//! One body's draw clock: how long the press has been held, and the edge
//! on which it looses — the button coming up, or the strain running out.
//! Both sides run one — ticks on the server, frame seconds on the client —
//! and only the server's edge looses anything.

use super::rows::Draw;

/// What the clock is told about the press each step.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Press<'a> {
    /// The press is the bow's and the button is down: draw, under this
    /// bow's authored draw.
    Held(&'a Draw),
    /// The bow is still in hand and the button came up — the draw looses.
    Released,
    /// The press can no longer belong to the bow: it left the hand (a
    /// hotbar scroll, a hand swap), the body died or went spectator. The
    /// draw is CANCELLED — no arrow — and the bow returns to rest.
    Lost,
}

/// Where the clock is between presses.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum State {
    /// No press.
    Idle,
    /// Drawing for this many ticks (a fraction on the frame clock).
    Drawing(f32),
    /// The strain loosed this press; the button has not come up yet.
    Spent,
}

#[derive(Default, Clone, Debug, PartialEq)]
pub struct Clock {
    held: f32,
    drawing: bool,
    /// The strain already loosed this press; nothing more until the button
    /// comes up, or a held button would fire every strain window.
    spent: bool,
    /// The full draw of the press being measured, in ticks — what a
    /// release caps at.
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

    /// Advance by `dt_ticks` under `press`. Answers the draw an arrow
    /// LOOSES at — `Some(ticks)` on a release, capped at the full draw, or
    /// at full when the strain window runs out with the button still down
    /// — else `None`. A press shorter than a tick looses nothing: that is
    /// a tap, not a draw. A LOST press looses nothing either: it resets.
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
