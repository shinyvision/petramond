//! The swing: started when the foe is in reach and in front, landed at the item's impact tick
//! through the target's damage funnel (the skeleton named as attacker, so knockback,
//! retaliation and a raised player shield all see a real mob hit), then played out.

use mod_sdk::*;

use super::super::geometry::{gap, within_arc};
use super::super::keys::SKELETON;
use super::super::kit::Swing;
use super::super::presence::Play;
use super::super::Skeletons;
use super::{clear_line, upright, Act, Fight, Foe, Turn};

const START_ARC_DEG: f32 = 50.0;
const LAND_ARC_DEG: f32 = 100.0;
const REACH_SLACK: f32 = 0.3;
/// Upward share of a weapon's extra shove.
const SHOVE_LIFT: f32 = 0.25;
const CHEST: f32 = 0.6;

impl Turn<'_> {
    pub(super) fn in_melee_reach(&self, foe: &Foe) -> bool {
        self.kit
            .swing
            .as_ref()
            .is_some_and(|s| gap(&upright(self.me), &foe.body) <= s.melee.reach)
    }

    /// Starts a swing at a foe in reach. `false` when the foe is out of reach, so the turn is free
    /// for the bow.
    pub(super) fn start_swing(&mut self, fight: &mut Fight, foe: &Foe) -> bool {
        let Some(swing) = &self.kit.swing else {
            return false;
        };
        if gap(&upright(self.me), &foe.body) > swing.melee.reach {
            return false;
        }
        self.hold = true;
        let facing = within_arc(self.me.yaw, self.me.pos, foe.body.feet, START_ARC_DEG);
        if facing && !fight.guard.raised {
            let cooldown = u64::from(swing.melee.cooldown);
            fight.act = Act::Swing {
                impact: self.now + u64::from(swing.melee.windup),
                until: self.now + cooldown,
                landed: false,
            };
            fight.ready_at = self.now + cooldown;
            self.plays.push(Play {
                clip: &swing.melee.clip,
                hold_at: None,
            });
        }
        true
    }

    pub(super) fn swinging(
        &mut self,
        fight: &mut Fight,
        foe: Option<&Foe>,
        shoves: &mut Vec<(PlayerId, [f32; 3])>,
    ) {
        self.hold = true;
        let (
            Act::Swing {
                impact,
                until,
                landed,
            },
            Some(swing),
        ) = (fight.act, &self.kit.swing)
        else {
            fight.act = Act::Idle;
            return;
        };
        if !landed && self.now >= impact {
            if let Some(foe) = foe {
                self.land(swing, foe, shoves);
            }
            fight.act = Act::Swing {
                impact,
                until,
                landed: true,
            };
        } else if landed && (self.now >= until || !self.playing(&swing.melee.clip)) {
            self.stop(&swing.melee.clip);
            fight.act = Act::Idle;
        }
    }

    /// The impact: it lands if the foe is still in reach, in front and in the open.
    fn land(&self, swing: &Swing, foe: &Foe, shoves: &mut Vec<(PlayerId, [f32; 3])>) {
        let mine = upright(self.me);
        let theirs = foe.body;
        if gap(&mine, &theirs) > swing.melee.reach + REACH_SLACK
            || !within_arc(self.me.yaw, self.me.pos, foe.body.feet, LAND_ARC_DEG)
            || !clear_line(mine.at(CHEST), theirs.at(CHEST))
        {
            return;
        }
        match foe.id {
            EntityRef::Player(id) => damage_player(
                id,
                swing.melee.damage.round() as i32,
                Some(mine.at(0.5)),
                Some(EntityRef::Mob(self.me.id)),
            ),
            EntityRef::Mob(id) => damage_mob(
                id,
                swing.melee.damage,
                Some(mine.at(0.5)),
                Some(EntityRef::Mob(self.me.id)),
            ),
        }
        let EntityRef::Player(victim) = foe.id else {
            return;
        };
        let push = swing.melee.knockback;
        if push > 0.0 {
            let (dx, dz) = (
                (foe.body.feet[0] - self.me.pos[0]) as f32,
                (foe.body.feet[2] - self.me.pos[2]) as f32,
            );
            let len = dx.hypot(dz).max(1.0e-4);
            shoves.push((
                victim,
                [dx / len * push, push * SHOVE_LIFT, dz / len * push],
            ));
        }
    }
}

/// The extra shove a weapon owes lands only on a hit that went through: this runs after every
/// other `player_damage_pre` handler, and one that refuses the hit ends the dispatch first.
pub fn on_player_damage(sk: &mut Skeletons, payload: &EventPayload) -> Outcome {
    let EventPayload::PlayerDamagePre {
        amount,
        source: DamageSource::MobAttack { key },
        ..
    } = payload
    else {
        return Outcome::Continue;
    };
    if key != SKELETON || *amount <= 0 {
        return Outcome::Continue;
    }
    let Some(victim) = player_state().id else {
        return Outcome::Continue;
    };
    if let Some(at) = sk.shoves.iter().position(|(p, _)| *p == victim) {
        let (_, impulse) = sk.shoves.remove(at);
        apply_knockback_to(victim, impulse);
    }
    Outcome::Continue
}
