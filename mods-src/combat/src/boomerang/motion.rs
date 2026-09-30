use super::rows::Row;
use crate::body::TICK_SECONDS;
use crate::charge::{self, Press, State};
use mod_sdk::ItemId;

#[derive(Default)]
pub(crate) struct Clock {
    draw: charge::Clock,
    item: Option<ItemId>,
    throwing: Option<Throw>,
}

struct Throw {
    item: ItemId,
    ticks: u32,
    elapsed: f32,
    released: bool,
}

pub(super) enum Pose {
    Draw(f32),
    Throw(f32),
}

impl Clock {
    pub fn item(&self) -> Option<ItemId> {
        self.throwing.as_ref().map(|t| t.item).or(self.item)
    }

    pub(super) fn pose(&self) -> Option<Pose> {
        if let Some(t) = &self.throwing {
            return Some(Pose::Throw(t.elapsed));
        }
        match self.draw.state() {
            State::Drawing(ticks) => Some(Pose::Draw(ticks)),
            _ => None,
        }
    }

    pub(super) fn step(
        &mut self,
        row: Option<&Row>,
        held: Option<ItemId>,
        alive: bool,
        press: bool,
        dt_ticks: f32,
    ) -> Option<u32> {
        if !alive {
            *self = Self::default();
            return None;
        }
        if let Some(t) = &mut self.throwing {
            if (!t.released && held != Some(t.item)) || held.is_some_and(|id| id != t.item) {
                *self = Self::default();
                return None;
            }
            let row = row?;
            t.elapsed += dt_ticks.max(0.0) * TICK_SECONDS;
            let release =
                (!t.released && t.elapsed >= row.motion.release_time()).then_some(t.ticks);
            t.released |= release.is_some();
            if t.elapsed >= row.motion.throw_length() {
                self.throwing = None;
            }
            return release;
        }
        let Some(row) = row.filter(|r| held == Some(r.id)) else {
            *self = Self::default();
            return None;
        };
        if self.item != Some(row.id) {
            self.draw = charge::Clock::default();
            self.item = Some(row.id);
        }
        if let Some(ticks) = self.draw.step(
            if press {
                Press::Held(&row.draw)
            } else {
                Press::Released
            },
            dt_ticks,
        ) {
            self.throwing = Some(Throw {
                item: row.id,
                ticks,
                elapsed: 0.0,
                released: false,
            });
        }
        None
    }
}
