//! What dripstone does to bodies: a falling piece strikes what it lands on,
//! and a stalagmite doubles the damage of a fall onto it. Both ride the
//! engine's ordinary damage pipeline — the pack only names the amount.

use mod_sdk::*;

use super::{Dripstone, POINTED_ITEM};

/// Damage a falling piece deals per m/s of arrival speed, and its bounds in
/// half-hearts. A one-block drop lands at ~6 m/s (three half-hearts); a
/// nine-block drop caps out.
const IMPACT_PER_SPEED: f32 = 0.5;
const IMPACT_MIN: f32 = 2.0;
const IMPACT_MAX: f32 = 12.0;
/// A fall onto a spike hurts this many times over — and at least this much.
const SPIKE_FACTOR: i32 = 2;
const SPIKE_MIN: i32 = 2;

/// A flying piece of pointed dripstone struck something: a body takes
/// impact damage; the piece drops as an item either way (a shattered
/// stalactite is still dripstone). Anything else in flight is not ours.
pub fn on_projectile_hit(payload: &mut EventPayload) -> Outcome {
    let EventPayload::ProjectileHit {
        entity,
        target,
        pos,
        vel,
        fate,
    } = payload
    else {
        return Outcome::Continue;
    };
    let Some(item) = item_entity(*entity) else {
        return Outcome::Continue;
    };
    if item.stack.item != POINTED_ITEM {
        return Outcome::Continue;
    }
    let speed = (vel[0] * vel[0] + vel[1] * vel[1] + vel[2] * vel[2]).sqrt();
    let amount = impact_damage(speed);
    match target {
        ProjectileTarget::Mob(mob) => damage_mob(*mob, amount, Some(*pos), None),
        ProjectileTarget::Player(victim) => {
            damage_player(*victim, amount.round() as i32, Some(*pos), None)
        }
        ProjectileTarget::Block { .. } => {}
    }
    *fate = ProjectileFate::Drop;
    Outcome::Continue
}

/// A player's fall damage doubles when the block under their feet is a
/// stalagmite. The dispatch names its victim, so the acting snapshot is the
/// player who landed.
pub fn on_player_damage(d: &Dripstone, payload: &mut EventPayload) -> Outcome {
    let EventPayload::PlayerDamagePre {
        amount,
        source: DamageSource::Fall,
        ..
    } = payload
    else {
        return Outcome::Continue;
    };
    if spike_under(d, player_state().pos) {
        *amount = spiked(*amount);
    }
    Outcome::Continue
}

/// The same rule for a mob.
pub fn on_mob_damage(d: &Dripstone, payload: &mut EventPayload) -> Outcome {
    let EventPayload::MobDamagePre {
        mob_id,
        amount,
        source: DamageSource::Fall,
        ..
    } = payload
    else {
        return Outcome::Continue;
    };
    if mob_info(*mob_id).is_some_and(|m| spike_under(d, m.pos)) {
        *amount = spiked(amount.round() as i32) as f32;
    }
    Outcome::Continue
}

/// Whether the cell under a body standing at `feet` is a stalagmite. The
/// probe sits just below the feet plane, which rests ON the block's top.
fn spike_under(d: &Dripstone, feet: [f64; 3]) -> bool {
    let c = [
        feet[0].floor() as i32,
        (feet[1] - 0.05).floor() as i32,
        feet[2].floor() as i32,
    ];
    get_block(c) == Some(d.stalagmite)
}

/// Impact damage by arrival speed: linear, floored so a short drop still
/// hurts, capped so a long one is survivable.
pub fn impact_damage(speed: f32) -> f32 {
    (speed * IMPACT_PER_SPEED).clamp(IMPACT_MIN, IMPACT_MAX)
}

/// A fall's damage once a spike is under it.
fn spiked(amount: i32) -> i32 {
    (amount * SPIKE_FACTOR).max(SPIKE_MIN)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The curve is what makes the height of a stalactite matter: strictly
    /// more damage for a faster arrival between the floor and the cap, and
    /// never outside them.
    #[test]
    fn impact_damage_grows_with_speed_between_its_bounds() {
        assert_eq!(impact_damage(0.0), IMPACT_MIN);
        assert_eq!(impact_damage(1000.0), IMPACT_MAX);
        let mut last = impact_damage(IMPACT_MIN / IMPACT_PER_SPEED);
        let mut v = IMPACT_MIN / IMPACT_PER_SPEED + 1.0;
        while v < IMPACT_MAX / IMPACT_PER_SPEED {
            let now = impact_damage(v);
            assert!(now > last, "{v} m/s: {now} <= {last}");
            last = now;
            v += 1.0;
        }
    }
}
