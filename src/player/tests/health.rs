use super::*;

#[test]
fn health_damage_and_restore_clamp_to_the_valid_range() {
    use petramond_world::damage::Immunity;
    let mut pl = p(WorldPos::new(0.0, 64.0, 0.0));
    assert_eq!(pl.health(), MAX_HEALTH, "starts at full health");
    assert!(pl.apply_damage(3, Immunity::PLAYER));
    assert_eq!(pl.health(), MAX_HEALTH - 3);
    assert!(!pl.apply_damage(0, Immunity::PLAYER));
    assert_eq!(pl.health(), MAX_HEALTH - 3);
    assert!(
        !pl.apply_damage(1000, Immunity::PLAYER),
        "the active i-frame window rejects damage"
    );
    for _ in 0..petramond_world::damage::PLAYER_DAMAGE_IFRAME_TICKS {
        pl.tick_damage_immunity();
    }
    assert!(pl.apply_damage(1000, Immunity::PLAYER));
    assert_eq!(pl.health(), 0);
    pl.set_health(1000);
    assert_eq!(pl.health(), MAX_HEALTH);
    pl.set_health(-5);
    assert_eq!(pl.health(), 0);
}

#[test]
fn damage_over_time_neither_checks_nor_grants_the_immunity_window() {
    use petramond_world::damage::Immunity;
    let mut pl = p(WorldPos::new(0.0, 64.0, 0.0));
    assert!(
        pl.apply_damage(2, Immunity::PLAYER),
        "the ordinary hit opens a window"
    );
    assert!(pl.is_damage_immune());
    assert!(
        pl.apply_damage(1, Immunity::Exempt),
        "a burn tick lands under an active window"
    );
    assert_eq!(pl.health(), MAX_HEALTH - 3);

    for _ in 0..petramond_world::damage::PLAYER_DAMAGE_IFRAME_TICKS {
        pl.tick_damage_immunity();
    }
    assert!(!pl.is_damage_immune());
    assert!(pl.apply_damage(1, Immunity::Exempt));
    assert!(
        !pl.is_damage_immune(),
        "a burn tick opens no window of its own"
    );
    assert!(
        pl.apply_damage(2, Immunity::PLAYER),
        "so the next ordinary hit lands at once"
    );
    assert_eq!(pl.health(), MAX_HEALTH - 6);
}

#[test]
fn status_effects_fire_on_interval_boundaries_and_expire() {
    use petramond_world::effect::{Effect, EffectBehavior};
    let EffectBehavior::Regen { interval, .. } = Effect::Regeneration.def().behavior else {
        panic!("regeneration is an interval-heal behavior");
    };

    let mut pl = p(WorldPos::new(0.0, 64.0, 0.0));
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

    pl.apply_effect(Effect::Regeneration, 10);
    pl.apply_effect(Effect::Regeneration, interval * 5);
    assert_eq!(pl.effects()[0].remaining, interval * 5);
    pl.apply_effect(Effect::Regeneration, 0);
    assert!(pl.effects().is_empty(), "zero ticks removes the effect");

    pl.set_health(MAX_HEALTH);
    pl.heal(5);
    assert_eq!(pl.health(), MAX_HEALTH, "healing clamps at full");
    pl.set_health(0);
    pl.heal(5);
    assert_eq!(pl.health(), 0, "healing never resurrects");
}
