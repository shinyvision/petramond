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
//! nothing about a bow: the draw rides the generic body seams (the body's
//! draw clip, the strain tremor on the held pose and the bow shoulder, the
//! held DISPLAY, the speed and denial claims) and the arrow rides the
//! generic launched-item primitive.

mod launch;
mod rows;
#[cfg(test)]
mod tests;

pub use crate::charge::{Clock, Press, State};
pub use rows::{BowRow, Rows};

use crate::body::BodyClocks;
use crate::claims::{Body, Claims, Rule};
use mod_sdk::*;
use std::rc::Rc;

/// THIRD PERSON, the draw: both arms up and the string hand coming back to
/// the jaw, scrubbed by how far the draw has come (an engine body clip in
/// the body's main claim slot). FIRST PERSON plays nothing: the bow must not
/// move in the hand while it charges (her call, twice — a raise into a draw
/// pose read as the bow wandering). The pull frames alone show the draw;
/// only the strain moves it.
const DRAW_3P: &str = "petramond:body_bow_draw";
const DRAW_SLOT: &str = "main_claim";

#[cfg(test)]
pub(crate) const CLIPS: [(&str, &str); 1] = [(rig::PLAYER_BODY, DRAW_3P)];

const SHAKE_PX: f32 = 0.9;
const SHAKE_DEG: f32 = 2.0;
const SHAKE_HZ: f32 = 0.45;

#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Bow<'a> {
    holds_press: bool,
    drawing: bool,
    draw: f32,
    strain: f32,
    row: Option<&'a BowRow>,
}

impl Bow<'_> {
    fn shake(&self) -> [f32; 2] {
        let strain_ticks = self.row.map_or(0, |row| row.draw.strain_ticks);
        if self.strain <= 0.0 || strain_ticks == 0 {
            return [0.0; 2];
        }
        let grow = (self.strain / strain_ticks as f32).clamp(0.0, 1.0);
        let t = self.strain * SHAKE_HZ * std::f32::consts::TAU;
        [t.sin() * grow, (t * 1.7 + 1.0).cos() * grow]
    }

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

    fn speed_scale(&self) -> f32 {
        match self.row {
            Some(row) if self.drawing => row.draw.speed_scale,
            _ => 1.0,
        }
    }

    fn pose(&self) -> Option<HeldPose> {
        self.drawing.then(|| {
            let [sx, sy] = self.shake();
            HeldPose {
                first_person: HeldPoseData {
                    rotation: [sy * SHAKE_DEG, 0.0, sx * SHAKE_DEG],
                    translation: [sx * SHAKE_PX, sy * SHAKE_PX, 0.0],
                },
                third_person: HeldPoseData::IDENTITY,
            }
        })
    }

    fn arms(&self) -> Vec<BonePoseData> {
        let [sx, sy] = self.shake();
        if !self.drawing || (sx == 0.0 && sy == 0.0) {
            return Vec::new();
        }
        vec![BonePoseData {
            bone: bone::MAIN_SHOULDER.to_string(),
            rotation: [sy * SHAKE_DEG, 0.0, sx * SHAKE_DEG],
            translation: [0.0; 3],
            mode: BonePoseMode::Compose,
        }]
    }

    fn plays(&self) -> Vec<AnimatorPlay> {
        self.drawing
            .then(|| AnimatorPlay::scrubbed(rig::PLAYER_BODY, DRAW_SLOT, DRAW_3P, self.draw))
            .into_iter()
            .collect()
    }

    pub fn claims(&self) -> Claims {
        Claims {
            holds_press: self.holds_press,
            speed: self.speed_scale(),
            denied: self.denied(),
            display: [self.display().map(str::to_owned), None],
            main: self.pose(),
            bones: self.arms(),
            plays: self.plays(),
            ..Default::default()
        }
    }
}

pub fn bow_of<'a>(rows: &'a Rows, state: &PlayerSnapshot, press: bool, clock: State) -> Bow<'a> {
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

pub struct BowRule {
    rows: Rc<Rows>,
}

impl BowRule {
    pub fn new(rows: Rc<Rows>) -> BowRule {
        BowRule { rows }
    }

    fn has_arrow(&self, player: PlayerId) -> bool {
        player_inventory(player)
            .into_iter()
            .flatten()
            .flatten()
            .any(|stack| self.rows.arrow_named(&stack.item).is_some())
    }
}

impl Rule for BowRule {
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
        if let (Some(ticks), Some(row), true) = (loosed, row, authority) {
            launch::loose(&self.rows, row, player, state, ticks);
        }
    }

    fn claims(&self, body: &Body) -> Claims {
        bow_of(&self.rows, body.state, body.press, body.clocks.draw.state()).claims()
    }
}
