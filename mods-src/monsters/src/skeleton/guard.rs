//! The shield and what reaches a skeleton. A shield is raised and lowered on random spans while a
//! foe is close. A hit from in
//! front of a raised shield: the block sounds, the shield jolts, and nothing else happens. A hit
//! that names no origin has no direction to judge and is never refused by the shield.

use mod_sdk::*;

use super::combat::wanted;
use super::geometry::within_arc;
use super::keys::BLOCK_SOUND;
use super::kit::Guard;
use super::presence::{self, Play};
use super::Skeletons;

/// Ticks the impact clip replaces the guard pose.
const RECOIL_TICKS: u64 = 6;

#[derive(Default)]
pub struct GuardClock {
    pub raised: bool,
    until: u64,
    /// While set, the impact clip plays in place of the guard pose.
    pub recoil_until: Option<u64>,
}

impl GuardClock {
    /// Flips the shield when its span runs out while a foe is close; a raise waits for a swing to
    /// finish, and with no foe close the shield comes down.
    pub fn cycle(
        &mut self,
        now: u64,
        guard: &Guard,
        engaged: bool,
        swinging: bool,
        roll: impl FnOnce() -> u64,
    ) {
        if !engaged {
            self.raised = false;
            self.until = 0;
            return;
        }
        if now < self.until || (swinging && !self.raised) {
            return;
        }
        self.raised = !self.raised;
        let [lo, hi] = if self.raised {
            guard.raise
        } else {
            guard.lower
        };
        let span = u64::from(hi - lo) + 1;
        self.until = now + u64::from(lo) + roll() % span;
    }
}

pub fn on_damage(sk: &mut Skeletons, payload: &EventPayload) -> Outcome {
    let EventPayload::MobDamagePre {
        mob_id,
        kind,
        origin,
        ..
    } = payload
    else {
        return Outcome::Continue;
    };
    if *kind != sk.kind {
        return Outcome::Continue;
    }
    let Some(body) = sk.bodies.get_mut(mob_id) else {
        return Outcome::Continue;
    };
    let kit = sk.kits.kit(body.loadout);
    let (Some(shield), Some(origin)) = (&kit.shield, origin) else {
        return Outcome::Continue;
    };
    if !body.fight.guard.raised {
        return Outcome::Continue;
    }
    let Some(me) = mob_info(*mob_id) else {
        return Outcome::Continue;
    };
    if !within_arc(me.yaw, me.pos, *origin, shield.guard.arc_deg) {
        return Outcome::Continue;
    }
    let sound = shield.guard.sound.as_deref().unwrap_or(BLOCK_SOUND);
    let [x, y, z] = me.pos;
    emit_sound(sound, Some([x, y + f64::from(me.height * 0.6), z]));
    body.fight.guard.recoil_until = Some(current_tick() + RECOIL_TICKS);
    let mut ops = Vec::new();
    let layers = wanted(kit, &body.fight, me.moving);
    let jolt = Play {
        clip: &shield.guard.impact_clip,
        hold_at: None,
    };
    body.presence.frame(*mob_id, &layers, &[jolt], &mut ops);
    presence::send(ops);
    Outcome::Cancel
}
