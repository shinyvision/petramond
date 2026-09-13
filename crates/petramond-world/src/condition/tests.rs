use super::*;

const INTERVAL: u32 = 10;

fn catalog() -> &'static [ConditionDef] {
    parse_test_catalog(&[r#"{"conditions": [
        {"condition": "test:hot", "stages": [
            {"stage": "low", "damage": {"amount": 1, "interval": 10}},
            {"stage": "high", "damage": {"amount": 2, "interval": 10},
             "cools_to": "low", "cools_after": 0.5}
        ]},
        {"condition": "test:wet", "stages": [{"stage": "damp"}]},
        {"condition": "test:sting", "stages": [
            {"stage": "calm"},
            {"stage": "sharp", "damage": {"amount": 1, "interval": 10}}
        ]}
    ]}"#])
    .expect("synthetic conditions load")
}

fn tick(body: &mut BodyConditions, defs: &[ConditionDef]) -> Vec<ConditionPulse> {
    let mut out = Vec::new();
    body.tick(defs, |p| out.push(p));
    out
}

#[test]
fn refreshing_a_condition_never_postpones_its_next_pulse() {
    let defs = catalog();
    let hot = &defs[0];
    let mut body = BodyConditions::default();
    let mut pulses = Vec::new();
    for t in 1..=INTERVAL * 5 {
        body.apply(hot, 1, 120);
        if !tick(&mut body, defs).is_empty() {
            pulses.push(t);
        }
    }
    assert_eq!(
        pulses,
        (INTERVAL..=INTERVAL * 5)
            .step_by(INTERVAL as usize)
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_stronger_stage_cools_to_its_weaker_stage_and_lingers_until_fuel_ends() {
    let defs = catalog();
    let mut body = BodyConditions::default();
    body.apply(&defs[0], 1, 40);
    let mut amounts = Vec::new();
    for _ in 0..40 {
        amounts.extend(tick(&mut body, defs).into_iter().map(|p| p.amount));
    }
    assert_eq!(
        amounts,
        vec![2, 2, 1, 1],
        "damage follows the current stage on a shared clock"
    );
    assert!(body.active().is_empty());
}

#[test]
fn applications_compose_without_shortening_or_downgrading() {
    let defs = catalog();
    let hot = &defs[0];
    let id = hot.id;
    let mut body = BodyConditions::default();
    body.apply(hot, 1, 100);
    for _ in 0..INTERVAL / 2 {
        tick(&mut body, defs);
    }
    let before = body.get(id).unwrap().clone();
    body.apply(hot, 0, 5);
    assert_eq!(body.get(id), Some(&before));
    body.apply(hot, 0, 200);
    assert_eq!(body.get(id).unwrap().stage(), 1);
    for _ in 1..INTERVAL - INTERVAL / 2 {
        assert!(tick(&mut body, defs).is_empty());
    }
    assert!(
        !tick(&mut body, defs).is_empty(),
        "extending fuel cannot postpone the next pulse"
    );
    body.cool(id, 50);
    assert_eq!(body.get(id).unwrap().stage(), 0);
    body.cool(id, u32::MAX);
    assert!(body.get(id).is_none());
}

#[test]
fn cooling_consumes_time_without_moving_the_pulse_boundary() {
    let defs = catalog();
    let hot = &defs[0];
    let mut body = BodyConditions::default();
    body.apply(hot, 0, 100);
    for _ in 0..3 {
        tick(&mut body, defs);
    }
    body.cool(hot.id, 20);
    for _ in 4..INTERVAL {
        assert!(tick(&mut body, defs).is_empty());
    }
    assert_eq!(tick(&mut body, defs).len(), 1);
}

#[test]
fn upgrading_from_a_harmless_stage_starts_a_full_interval() {
    let defs = catalog();
    let sting = &defs[2];
    let mut body = BodyConditions::default();
    body.apply(sting, 0, 100);
    for _ in 0..5 {
        assert!(tick(&mut body, defs).is_empty());
    }
    body.apply(sting, 1, 50);
    for _ in 1..INTERVAL {
        assert!(
            tick(&mut body, defs).is_empty(),
            "an upgrade pulses no earlier than a fresh condition"
        );
    }
    assert_eq!(tick(&mut body, defs).len(), 1);
}

#[test]
fn conditions_tick_independently_in_id_order_and_clear_individually() {
    let defs = catalog();
    let mut body = BodyConditions::default();
    body.apply(&defs[1], 0, 30);
    body.apply(&defs[0], 0, 30);
    assert_eq!(
        body.active()
            .iter()
            .map(|c| c.condition)
            .collect::<Vec<_>>(),
        vec![defs[0].id, defs[1].id]
    );
    body.clear(defs[0].id);
    for _ in 0..INTERVAL {
        assert!(
            tick(&mut body, defs).is_empty(),
            "a harmless stage pulses nothing"
        );
    }
    assert_eq!(body.get(defs[1].id).unwrap().elapsed(), INTERVAL);
    body.clear(defs[1].id);
    assert!(body.active().is_empty());
}

#[test]
fn a_cooling_chain_must_step_down_to_an_earlier_stage() {
    let err = parse_test_catalog(&[r#"{"conditions": [{"condition": "test:bad", "stages": [
        {"stage": "a", "cools_to": "b", "cools_after": 0.5}, {"stage": "b"}
    ]}]}"#])
    .unwrap_err();
    assert!(err.contains("earlier stage"), "{err}");
}
