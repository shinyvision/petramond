//! The bow: the whole draw-and-loose law, and the rig facts behind it.
//!
//! A bow in the MAIN hand takes the use press and DRAWS for as long as the
//! button is held: the row's `draw_ticks` to full, shown through the bow's
//! pull frames (the last is always full) while the body slows and the
//! hands are committed. A full draw held on STRAINS — the bow shakes,
//! harder the longer — and after the row's `strain_ticks` looses itself.
//! Letting go LOOSES an arrow from the pack: launched from the eye along
//! the look, faster the longer the draw ([`launch`]). What the arrow does
//! when it lands is decided by the speed it ARRIVED with, against the
//! arrow row's own damage rungs ([`rows::ArrowRow::damage_at`]); a weak
//! shot also falls to the ground within a few blocks, because it left
//! slowly.
//!
//! Every tuned NUMBER is row data ([`rows`]): a second bow or arrow is a
//! JSON row. What stays here is the rig — nock offsets, poses, the shake —
//! which are facts of the art, not of a tier.
//!
//! [`bow_of`] is a pure function of the actor snapshot, the press and the
//! clock's state, and the server tick, the client frame and the release
//! all read it, which is what makes a prediction that disagrees with the
//! authority impossible rather than merely unlikely. The engine knows
//! nothing about a bow: the draw rides the generic body seams (the held
//! pose, the arm bones, the held DISPLAY, the speed and denial claims) and
//! the arrow rides the generic launched-item primitive.

mod clock;
mod launch;
mod rows;
#[cfg(test)]
mod tests;

pub use clock::{Clock, Press, State};
pub use rows::{BowRow, Rows};

use crate::body::BodyClocks;
use crate::claims::{Body, Claims, Rule};
use mod_sdk::*;
use std::rc::Rc;

/// FIRST PERSON, drawing: the AUTHORED hold, untouched. The pull frames
/// alone show the draw — the bow must not move in the hand while it
/// charges (her call, twice: a raise into a draw pose read as the bow
/// wandering). Only the strain tremor moves it, about this rest.
const DRAW_1P: HeldPoseData = HeldPoseData::IDENTITY;

/// THIRD PERSON, the bow arm: held out level in front, the elbow straight,
/// as a `Replace` stance on the MAIN hand's shoulder and elbow (the rig
/// cross-names the arms, hence `bone::MAIN_*`).
const DRAW_SHOULDER: [f32; 3] = [-85.0, 0.0, 0.0];
const DRAW_ELBOW: [f32; 3] = [0.0, 0.0, 0.0];

/// THIRD PERSON, the string arm: raised beside the bow arm, its elbow
/// folding back toward the cheek as the draw comes (the elbow folds about
/// its X); the fold is what reads as the draw on somebody else's body.
const STRING_SHOULDER: [f32; 3] = [-80.0, 0.0, 0.0];
const STRING_ELBOW_FULL: [f32; 3] = [90.0, 0.0, 0.0];

/// THIRD PERSON, the bow in the extended fist: the authored carry, lifted
/// a touch. The ARM carries the motion in third person; turning the item
/// too did the arm's work twice and sank the bow under the fist.
const DRAW_3P: HeldPoseData = HeldPoseData {
    rotation: [0.0, 0.0, 0.0],
    translation: [0.0, 2.0, 0.0],
};

/// The strain shake at full draw: pixels and degrees of jitter at the
/// peak, and how fast it trembles (cycles per tick).
const SHAKE_PX: f32 = 0.9;
const SHAKE_DEG: f32 = 2.0;
const SHAKE_HZ: f32 = 0.45;

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// What the bow is doing for one actor.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Bow<'a> {
    /// The use press is the bow's — drawing, or SPENT by the strain and
    /// inert until the button comes up. Either way no later rule gets it.
    holds_press: bool,
    /// A draw is showing. Everything below is meaningless while `false`.
    drawing: bool,
    /// How far the draw has come, `0..=1` (`1` = the row's full draw).
    draw: f32,
    /// Ticks held PAST full — the strain, `0..=strain_ticks`.
    strain: f32,
    /// The bow in the main hand, if any.
    row: Option<&'a BowRow>,
}

impl Bow<'_> {
    /// The strain tremor at this instant: a jitter that grows with the
    /// strain, `[x, y]` in `-1..=1`. Zero below full draw. A pure function
    /// of the clock, so both sides shake alike.
    fn shake(&self) -> [f32; 2] {
        let strain_ticks = self.row.map_or(0, |row| row.draw.strain_ticks);
        if self.strain <= 0.0 || strain_ticks == 0 {
            return [0.0; 2];
        }
        let grow = (self.strain / strain_ticks as f32).clamp(0.0, 1.0);
        let t = self.strain * SHAKE_HZ * std::f32::consts::TAU;
        [t.sin() * grow, (t * 1.7 + 1.0).cos() * grow]
    }

    /// Which sprite shows this draw, `0` = the bow's own art, `n` = the
    /// n-th pull frame: the frames spread evenly over the draw, the LAST
    /// reserved for the FULL draw — so a fully drawn bow is unmistakable.
    fn stage(&self) -> usize {
        let frames = self.row.map_or(0, |row| row.pull.len());
        if !self.drawing || frames == 0 {
            return 0;
        }
        if self.draw >= 1.0 {
            frames
        } else {
            (self.draw * frames as f32).floor() as usize
        }
    }

    /// The item the main hand DISPLAYS instead of the bow: a pull frame, or
    /// `None` for the bow's own art (rest, and the earliest draw). A frame
    /// the registry lacks holds the previous one.
    fn display(&self) -> Option<&str> {
        let stage = self.stage();
        let row = self.row?;
        row.pull[..stage]
            .iter()
            .rev()
            .find_map(|name| name.as_deref())
    }

    fn denied(&self) -> Vec<BodyAction> {
        if self.drawing {
            vec![BodyAction::Attack, BodyAction::Mine]
        } else {
            Vec::new()
        }
    }

    /// The land-speed multiplier to claim (`1.0` releases it).
    fn speed_scale(&self) -> f32 {
        match self.row {
            Some(row) if self.drawing => row.draw.speed_scale,
            _ => 1.0,
        }
    }

    /// The main hand's pose: the bow held for the draw, or `None` (the
    /// authored carry) at rest.
    fn pose(&self) -> Option<HeldPose> {
        self.drawing.then(|| {
            let [sx, sy] = self.shake();
            HeldPose {
                first_person: HeldPoseData {
                    rotation: [
                        DRAW_1P.rotation[0] + sy * SHAKE_DEG,
                        DRAW_1P.rotation[1],
                        DRAW_1P.rotation[2] + sx * SHAKE_DEG,
                    ],
                    translation: [
                        DRAW_1P.translation[0] + sx * SHAKE_PX,
                        DRAW_1P.translation[1] + sy * SHAKE_PX,
                        DRAW_1P.translation[2],
                    ],
                },
                third_person: DRAW_3P,
            }
        })
    }

    /// Both arms in the archer's stance — empty at rest, so the arms hang
    /// and swing normally. The bow arm is up from the first frame; the string
    /// arm's elbow folds with the draw.
    fn arms(&self) -> Vec<BonePoseData> {
        if !self.drawing {
            return Vec::new();
        }
        let hold = |bone: &str, rotation: [f32; 3]| BonePoseData {
            bone: bone.to_string(),
            rotation,
            translation: [0.0; 3],
            mode: BonePoseMode::Replace,
        };
        let [sx, sy] = self.shake();
        vec![
            hold(
                bone::MAIN_SHOULDER,
                [
                    DRAW_SHOULDER[0] + sy * SHAKE_DEG,
                    DRAW_SHOULDER[1],
                    DRAW_SHOULDER[2] + sx * SHAKE_DEG,
                ],
            ),
            hold(bone::MAIN_ELBOW, DRAW_ELBOW),
            hold(bone::OFF_SHOULDER, STRING_SHOULDER),
            hold(
                bone::OFF_ELBOW,
                lerp3([0.0; 3], STRING_ELBOW_FULL, self.draw),
            ),
        ]
    }

    /// Everything the bow claims about the body this tick, for the
    /// publisher to merge: nothing at all at rest, and — a press the strain
    /// spent — only the press itself, so nothing else takes it before the
    /// button comes up.
    pub fn claims(&self) -> Claims {
        Claims {
            holds_press: self.holds_press,
            speed: self.speed_scale(),
            denied: self.denied(),
            display: [self.display().map(str::to_owned), None],
            main: self.pose(),
            bones: self.arms(),
            ..Default::default()
        }
    }
}

/// The entire draw law as a pure function of the actor snapshot, whether
/// the press is the bow's (the composition's answer) and the draw clock.
pub fn bow_of<'a>(rows: &'a Rows, state: &PlayerSnapshot, press: bool, clock: State) -> Bow<'a> {
    // A spectator draws nothing; deciding it HERE releases every claim.
    let row = rows.bow(state.held).filter(|_| !state.spectator);
    let mut bow = Bow {
        holds_press: press,
        drawing: false,
        draw: 0.0,
        strain: 0.0,
        row,
    };
    if let (true, Some(row), State::Drawing(ticks)) = (press, row, clock) {
        let full = row.draw.full_ticks as f32;
        bow.drawing = true;
        bow.draw = (ticks / full).clamp(0.0, 1.0);
        bow.strain = (ticks - full).clamp(0.0, row.draw.strain_ticks as f32);
    }
    bow
}

/// The bow as one of the pack's rules: a bow in the MAIN hand, with an
/// arrow in the pack, takes a free press and draws.
pub struct BowRule {
    rows: Rc<Rows>,
}

impl BowRule {
    pub fn new(rows: Rc<Rows>) -> BowRule {
        BowRule { rows }
    }

    /// Whether `player` carries any arrow row — read on the PRESS only.
    fn has_arrow(&self, player: PlayerId) -> bool {
        player_inventory(player)
            .into_iter()
            .flatten()
            .flatten()
            .any(|stack| self.rows.arrow_named(&stack.item).is_some())
    }
}

impl Rule for BowRule {
    /// A bow with no arrow to loose has no draw to show: the press is not
    /// taken, on either side (the client reads its replicated inventory),
    /// and a later rule gets it instead.
    fn takes_press(&self, state: &PlayerSnapshot) -> bool {
        let Some(me) = state.id else {
            return false;
        };
        !state.spectator && self.rows.bow(state.held).is_some() && self.has_arrow(me)
    }

    fn step(
        &self,
        clocks: &mut BodyClocks,
        player: PlayerId,
        state: &PlayerSnapshot,
        press: bool,
        dt_ticks: f32,
        authority: bool,
    ) {
        let row = self
            .rows
            .bow(state.held)
            .filter(|_| !state.spectator && state.health > 0);
        let press = match (row, press) {
            (Some(row), true) => Press::Held(&row.draw),
            (Some(_), false) => Press::Released,
            (None, _) => Press::Lost,
        };
        let loosed = clocks.draw.step(press, dt_ticks);
        // Only the server's edge launches anything; a mirror shows the draw.
        if let (Some(ticks), Some(row), true) = (loosed, row, authority) {
            launch::loose(&self.rows, row, player, state, ticks);
        }
    }

    fn claims(&self, body: &Body) -> Claims {
        bow_of(&self.rows, body.state, body.press, body.clocks.draw.state()).claims()
    }
}
