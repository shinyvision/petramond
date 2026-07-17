use super::*;

#[test]
fn health_damage_and_restore_clamp_to_the_valid_range() {
    let mut pl = p(Vec3::new(0.0, 64.0, 0.0));
    assert_eq!(pl.health(), MAX_HEALTH, "starts at full health");
    assert!(pl.apply_damage(3));
    assert_eq!(pl.health(), MAX_HEALTH - 3);
    assert!(!pl.apply_damage(0)); // non-positive is a no-op
    assert_eq!(pl.health(), MAX_HEALTH - 3);
    assert!(
        !pl.apply_damage(1000),
        "the active i-frame window rejects damage"
    );
    for _ in 0..crate::damage::PLAYER_DAMAGE_IFRAME_TICKS {
        pl.tick_damage_immunity();
    }
    assert!(pl.apply_damage(1000)); // never below zero
    assert_eq!(pl.health(), 0);
    pl.set_health(1000); // restore clamps to the max
    assert_eq!(pl.health(), MAX_HEALTH);
    pl.set_health(-5);
    assert_eq!(pl.health(), 0);
}

#[test]
fn status_effects_fire_on_interval_boundaries_and_expire() {
    use crate::effect::{Effect, EffectBehavior};
    // Derive the cadence from the loaded row — the contract under test is the
    // boundary/expiry behavior, never the freely-editable interval/amount.
    let EffectBehavior::Regen { interval, .. } = Effect::Regeneration.def().behavior else {
        panic!("regeneration is an interval-heal behavior");
    };

    // The player owns WHEN a behavior fires (Game applies the consequences,
    // so damage can route through its funnel): boundaries land every
    // `interval` ticks, including one at expiry.
    let mut pl = p(Vec3::new(0.0, 64.0, 0.0));
    pl.apply_effect(Effect::Regeneration, interval * 2);
    let mut fired = 0;
    for _ in 0..interval {
        fired += pl.tick_effects().len();
    }
    assert_eq!(fired, 1, "the first boundary fires exactly once");
    for _ in 0..interval {
        fired += pl.tick_effects().len();
    }
    assert_eq!(fired, 2, "the expiry tick is itself a boundary");
    assert!(pl.effects().is_empty(), "the effect expired");

    // Re-applying overwrites the duration (in place); zero removes.
    pl.apply_effect(Effect::Regeneration, 10);
    pl.apply_effect(Effect::Regeneration, interval * 5);
    assert_eq!(pl.effects()[0].remaining, interval * 5);
    pl.apply_effect(Effect::Regeneration, 0);
    assert!(pl.effects().is_empty(), "zero ticks removes the effect");

    // The heal primitive the regen consequence lands through clamps at full
    // and never resurrects — respawn owns that transition.
    pl.set_health(MAX_HEALTH);
    pl.heal(5);
    assert_eq!(pl.health(), MAX_HEALTH, "healing clamps at full");
    pl.set_health(0);
    pl.heal(5);
    assert_eq!(pl.health(), 0, "healing never resurrects");
}
