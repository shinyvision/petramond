//! The bow: a draw starts when the foe is inside the bow's band with a clear line, holds at full
//! draw until the shot is clear (or is let down), and the release launches the ammo row on an
//! aimed, slightly scattered arc. No ammo is spent; the combat pack's arrow handler does the rest.

use mod_sdk::*;

use super::super::aim;
use super::super::geometry::{distance, segment_meets_box};
use super::super::kit::Bow;
use super::super::presence::Play;
use super::{clear_line, Act, Fight, Foe, Turn};

/// A drawn bow waits this long past full draw for a clear shot before it is let down.
const HOLD_PATIENCE: u64 = 40;
/// How long a let-down draw keeps the bow from drawing again.
const DRAW_BACKOFF: u64 = 10;
/// A drawn foe this far past the bow's reach is given up.
const DRAW_SLACK: f64 = 4.0;
const NOCK_HEIGHT: f32 = 0.8;
const NOCK_AHEAD: f64 = 0.35;
const AIM_HEIGHT: f32 = 0.55;

fn aim_point(foe: &Foe) -> [f64; 3] {
    foe.body.at(AIM_HEIGHT)
}

impl Turn<'_> {
    /// Whether the bow can reach `foe` from here.
    pub(super) fn within_band(&self, foe: &Foe) -> bool {
        self.kit.bow.as_ref().is_some_and(|bow| {
            let d = distance(self.me.pos, foe.body.feet);
            d >= f64::from(bow.ranged.min_range) && d <= f64::from(bow.ranged.max_range)
        })
    }

    pub(super) fn start_draw(&mut self, fight: &mut Fight, foe: &Foe) {
        let Some(bow) = &self.kit.bow else {
            return;
        };
        if distance(self.me.pos, foe.body.feet) > f64::from(bow.ranged.max_range)
            || !(self.retreating || self.within_band(foe))
            || !self.line_free(self.nock(foe), aim_point(foe), foe.id)
        {
            return;
        }
        let draw = u64::from(bow.ranged.draw);
        fight.act = Act::Draw {
            release: self.now + draw,
            give_up: self.now + draw + HOLD_PATIENCE,
        };
        self.hold = true;
        self.plays.push(Play {
            clip: &bow.ranged.clip_draw,
            hold_at: Some(draw as f32 * aim::TICK_SECONDS),
        });
    }

    pub(super) fn drawing(&mut self, fight: &mut Fight, foe: Option<&Foe>) {
        self.hold = true;
        let (Act::Draw { release, give_up }, Some(bow)) = (fight.act, &self.kit.bow) else {
            fight.act = Act::Idle;
            return;
        };
        let reach = f64::from(bow.ranged.max_range) + DRAW_SLACK;
        let Some(foe) = foe.filter(|f| distance(self.me.pos, f.body.feet) <= reach) else {
            fight.act = Act::Idle;
            fight.ready_at = self.now + DRAW_BACKOFF;
            return;
        };
        // Show one complete tick of the fully drawn bow before releasing it.
        if self.now <= release {
            return;
        }
        if self.loose(bow, foe) {
            self.plays.push(Play {
                clip: &bow.ranged.clip_loose,
                hold_at: None,
            });
            let rest = u64::from(bow.ranged.rest);
            fight.act = Act::Loose {
                until: self.now + rest,
                clip_live: true,
            };
            fight.ready_at = self.now + rest;
        } else if self.now >= give_up {
            fight.act = Act::Idle;
            fight.ready_at = self.now + DRAW_BACKOFF;
        }
    }

    pub(super) fn resting(&mut self, fight: &mut Fight) {
        let (Act::Loose { until, clip_live }, Some(bow)) = (fight.act, &self.kit.bow) else {
            fight.act = Act::Idle;
            return;
        };
        let clip = &bow.ranged.clip_loose;
        let clip_live = clip_live && self.playing(clip);
        fight.act = if self.now >= until {
            if clip_live {
                self.stop(clip);
            }
            Act::Idle
        } else {
            Act::Loose { until, clip_live }
        };
    }

    fn nock(&self, foe: &Foe) -> [f64; 3] {
        let [x, y, z] = self.me.pos;
        let (dx, dz) = (foe.body.feet[0] - x, foe.body.feet[2] - z);
        let len = dx.hypot(dz).max(1.0e-6);
        [
            x + dx / len * NOCK_AHEAD,
            y + f64::from(self.me.height * NOCK_HEIGHT),
            z + dz / len * NOCK_AHEAD,
        ]
    }

    /// Whether a shot from `from` to `to` meets no terrain and no other body.
    fn line_free(&self, from: [f64; 3], to: [f64; 3], target: EntityRef) -> bool {
        if !clear_line(from, to) {
            return false;
        }
        let mid = [
            0.5 * (from[0] + to[0]),
            0.5 * (from[1] + to[1]),
            0.5 * (from[2] + to[2]),
        ];
        let radius = (0.5 * distance(from, to) + 2.0) as f32;
        mobs_in_radius(mid, radius)
            .iter()
            .filter(|m| m.id != self.me.id && target != EntityRef::Mob(m.id))
            .all(|m| {
                let r = f64::from(m.half_width.max(m.half_length));
                let [x, y, z] = m.pos;
                let min = [x - r, y, z - r];
                let max = [x + r, y + f64::from(m.height), z + r];
                !segment_meets_box(from, to, min, max)
            })
    }

    fn loose(&self, bow: &Bow, foe: &Foe) -> bool {
        let from = self.nock(foe);
        let at = aim_point(foe);
        if !self.line_free(from, at, foe.id) {
            return false;
        }
        let speed = bow.ranged.speed;
        let Some(shot) = aim::aim_leading(bow.flight, speed, from, at, foe.vel) else {
            return false;
        };
        let roll = rng_u64("skeleton_arrow") ^ self.me.id;
        let rolls = [splitmix64_mix(roll), splitmix64_mix(roll.rotate_left(29))];
        let dir = aim::scatter(shot.dir, bow.ranged.spread, rolls);
        launch_item(
            &bow.ranged.ammo,
            from,
            dir.map(|c| c * speed),
            Some(EntityRef::Mob(self.me.id)),
            &[(
                "petramond:item_entity",
                br#"{"pickup":false,"lifetime_ticks":2400}"#,
            )],
        );
        true
    }
}
