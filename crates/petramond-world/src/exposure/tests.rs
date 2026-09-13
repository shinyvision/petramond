use super::*;
use crate::condition::{parse_test_catalog, Pulse};
use crate::fluid::{ConditionGrant, CurrentProperties, FluidClimb, FluidContact, FluidMotion};

const INTERVAL: u32 = 10;
const GRANT: u32 = 120;
const HOT: ConditionId = ConditionId(0);

fn conditions() -> &'static [ConditionDef] {
    parse_test_catalog(&[r#"{"conditions": [{"condition": "test:hot", "stages": [
        {"stage": "low", "damage": {"amount": 1, "interval": 10}},
        {"stage": "high", "damage": {"amount": 1, "interval": 10},
         "cools_to": "low", "cools_after": 0.5}
    ]}]}"#])
    .expect("synthetic conditions load")
}

fn fluid(block: Block, contact: FluidContact) -> &'static FluidDef {
    Box::leak(Box::new(FluidDef {
        block,
        name: "test:fluid",
        delay: 5,
        drop_off: 1,
        renewable: false,
        quench: None,
        motion: FluidMotion {
            speed_scale: 1.0,
            accel: 1.0,
            friction: 0.5,
            rise: 1.0,
            sink: 1.0,
            vertical_accel: 1.0,
            entry_friction: 0.0,
            probe_fraction: 0.5,
            probe_offset: 0.0,
            climb: FluidClimb::Jump,
        },
        current: CurrentProperties::default(),
        splash: None,
        contact,
        medium: serde_json::from_str(
            r#"{"fog_color": [0, 0, 0], "fog_start": 0, "fog_end": 1,
                "volume_tint": [1, 1, 1], "surface_tint": [1, 1, 1], "surface_alpha": 1}"#,
        )
        .expect("synthetic medium"),
    }))
}

/// Deals contact damage and applies the hot condition's strong stage. The block
/// ids only order the synthetic fluids; no row of theirs is read.
fn hazard() -> &'static FluidDef {
    fluid(
        Block::Grass,
        FluidContact {
            damage: Some(Pulse {
                amount: 3,
                interval: INTERVAL,
            }),
            applies: Some(ConditionGrant {
                condition: HOT,
                stage: 1,
                ticks: GRANT,
            }),
            ..Default::default()
        },
    )
}

fn sting(interval: u32) -> &'static FluidDef {
    fluid(
        Block::Stone,
        FluidContact {
            damage: Some(Pulse {
                amount: 1,
                interval,
            }),
            ..Default::default()
        },
    )
}

fn douse() -> &'static FluidDef {
    fluid(
        Block::Dirt,
        FluidContact {
            clears: &[HOT],
            ..Default::default()
        },
    )
}

/// `touched` in any order; the tick's contract is sorted and unique.
fn step(body: &mut BodyExposure, touched: &[&'static FluidDef]) -> Vec<ExposureDamage> {
    let mut touched = touched.to_vec();
    touched.sort_by_key(|f| f.block.id());
    touched.dedup_by_key(|f| f.block.id());
    let mut out = Vec::new();
    body.tick(Some(&touched), conditions(), &mut out);
    out
}

fn ticks_hit_by(hits: &[(u32, ExposureDamage)], source: ExposureSource) -> Vec<u32> {
    hits.iter()
        .filter(|(_, d)| d.source == source)
        .map(|(t, _)| *t)
        .collect()
}

fn every(interval: u32, from: u32, to: u32) -> Vec<u32> {
    (from..=to).step_by(interval as usize).collect()
}

#[test]
fn continuous_contact_runs_both_clocks_independently() {
    let (hazard, mut body, mut hits) = (hazard(), BodyExposure::default(), Vec::new());
    for t in 1..=GRANT * 2 {
        hits.extend(step(&mut body, &[hazard]).into_iter().map(|d| (t, d)));
    }
    assert_eq!(
        ticks_hit_by(&hits, ExposureSource::Fluid(hazard.block)),
        every(INTERVAL, 1, GRANT * 2)
    );
    assert_eq!(
        ticks_hit_by(&hits, ExposureSource::Condition(HOT)),
        every(INTERVAL, INTERVAL, GRANT * 2)
    );
}

#[test]
fn a_clearing_fluid_wins_the_condition_but_not_the_other_fluids_contact_damage() {
    let (hazard, douse) = (hazard(), douse());
    let mut body = BodyExposure::default();
    for _ in 1..INTERVAL {
        step(&mut body, &[hazard]);
    }
    assert!(
        step(&mut body, &[douse, hazard]).is_empty(),
        "the due pulse is cleared before it lands"
    );
    assert!(body.conditions().active().is_empty());
    let hits = step(&mut body, &[hazard, douse]);
    assert_eq!(
        hits.iter().map(|d| d.source).collect::<Vec<_>>(),
        [ExposureSource::Fluid(hazard.block)]
    );
    assert!(body.conditions().active().is_empty());
}

#[test]
fn a_body_in_a_clearing_fluid_refuses_later_grants_until_it_leaves() {
    let (douse, defs) = (douse(), conditions());
    let mut body = BodyExposure::default();
    assert!(body.apply(&defs[0], 0, GRANT));
    for _ in 0..3 {
        step(&mut body, &[douse]);
        assert!(body.conditions().active().is_empty());
        assert!(
            !body.apply(&defs[0], 1, GRANT),
            "a grant between exposure ticks is refused while doused"
        );
        assert!(body.conditions().active().is_empty());
    }
    step(&mut body, &[]);
    assert!(
        body.apply(&defs[0], 1, GRANT),
        "out of the fluid, grants land"
    );
    assert!(body.conditions().get(HOT).is_some());
}

#[test]
fn tolerated_fluids_are_not_felt_and_tolerated_conditions_are_refused() {
    let (hazard, douse, defs) = (hazard(), douse(), conditions());
    let blocks: &'static [Block] = Box::leak(Box::new([hazard.block, douse.block]));
    let mut dweller = BodyExposure::new(Tolerance {
        blocks,
        conditions: &[],
    });
    assert!(dweller.apply(&defs[0], 0, GRANT));
    assert!(step(&mut dweller, &[hazard, douse]).is_empty());
    assert!(
        dweller.conditions().get(HOT).is_some(),
        "a tolerated clearing fluid clears nothing"
    );

    let mut fireproof = BodyExposure::new(Tolerance {
        blocks: &[],
        conditions: &[HOT],
    });
    for _ in 0..INTERVAL * 2 {
        for hit in step(&mut fireproof, &[hazard]) {
            assert_eq!(hit.source, ExposureSource::Fluid(hazard.block));
        }
        assert!(!fireproof.apply(&defs[0], 1, GRANT));
        assert!(fireproof.conditions().active().is_empty());
    }
    fireproof.clear();
    assert!(
        !fireproof.apply(&defs[0], 1, GRANT),
        "a fresh life keeps the body's tolerance"
    );
}

#[test]
fn unknown_contact_skips_the_fluids_but_still_ticks_conditions() {
    let (hazard, douse, defs) = (hazard(), douse(), conditions());
    let mut body = BodyExposure::default();
    assert!(body.apply(&defs[0], 0, GRANT));
    let mut pulses = Vec::new();
    for _ in 0..INTERVAL {
        body.tick(None, defs, &mut pulses);
    }
    assert_eq!(
        pulses,
        [ExposureDamage {
            amount: 1,
            source: ExposureSource::Condition(HOT)
        }],
        "conditions pulse while contact is unknown"
    );

    step(&mut body, &[hazard, douse]);
    let mut out = Vec::new();
    body.tick(None, defs, &mut out);
    assert!(out.is_empty());
    assert!(
        !body.apply(&defs[0], 0, GRANT),
        "unknown contact keeps the last known doused set"
    );
}

#[test]
fn each_fluid_runs_its_own_contact_clock() {
    const STING: u32 = 3;
    let (hazard, sting, mut body, mut hits) =
        (hazard(), sting(STING), BodyExposure::default(), Vec::new());
    for t in 1..=INTERVAL * 2 {
        let touched: &[&'static FluidDef] = if t % 2 == 0 {
            &[sting, hazard]
        } else {
            &[hazard, sting]
        };
        hits.extend(step(&mut body, touched).into_iter().map(|d| (t, d)));
    }
    assert_eq!(
        ticks_hit_by(&hits, ExposureSource::Fluid(hazard.block)),
        every(INTERVAL, 1, INTERVAL * 2)
    );
    assert_eq!(
        ticks_hit_by(&hits, ExposureSource::Fluid(sting.block)),
        every(STING, 1, INTERVAL * 2)
    );
}

#[test]
fn briefly_leaving_and_reentering_cannot_hurry_contact_damage() {
    let (hazard, mut body) = (hazard(), BodyExposure::default());
    let contact = |hits: Vec<ExposureDamage>| {
        hits.iter()
            .any(|d| d.source == ExposureSource::Fluid(hazard.block))
    };
    assert!(contact(step(&mut body, &[hazard])));
    for i in 1..INTERVAL {
        let touched: &[&'static FluidDef] = if i % 2 == 0 { &[hazard] } else { &[] };
        assert!(!contact(step(&mut body, touched)));
    }
    assert!(contact(step(&mut body, &[hazard])));
}
