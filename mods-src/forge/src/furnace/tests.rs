use machine_core::Burner;

use super::*;

#[test]
fn a_full_cold_crucible_still_wants_fuel() {
    let full = State {
        units: CRUCIBLE_MAX,
        metal: "petramond:raw_iron".into(),
        ..State::default()
    };
    assert!(
        wants_heat(&full, false),
        "a full crucible must still call for the fire"
    );
    let empty = State::default();
    assert!(
        !wants_heat(&empty, false),
        "but an idle forge never eats coal"
    );
}

#[test]
fn the_fire_is_the_only_thing_the_mask_stages() {
    let spec = ForgingFurnaceSpec::default();
    assert_eq!(
        spec.parts_mask(&State::default()),
        0,
        "a cold forge shows no fire"
    );
    let lit = State {
        fire: burning(40),
        ..State::default()
    };
    assert_eq!(spec.parts_mask(&lit), PART_COALS);
}

#[test]
fn the_lever_frame_follows_the_phase() {
    let mut state = State::default();
    assert_eq!(lever_frame(&state), 0, "idle snaps the lever back up");

    state.phase = Phase::Pouring;
    for (ticks, want) in [(0, 0), (6, 3), (11, 6), (12, 7), (POUR_TICKS - 1, 7)] {
        state.phase_ticks = ticks;
        assert_eq!(lever_frame(&state), want, "pour tick {ticks}");
    }

    state.phase = Phase::Setting;
    state.phase_ticks = 0;
    assert_eq!(lever_frame(&state), 7, "the lever stays down while it sets");
}

/// Row follows the stream, not the phase. Row owns the droplet emitter.
/// Phase runs longer than the visible stream by the whole set. Go by phase alone and
/// droplets and spatter keep going for `SET_TICKS` after the metal has finished falling.
/// Row stays up while the tap's open or metal's airborne, and drops as soon as tail catches head.
#[test]
fn the_pour_row_outlives_the_phase_exactly_as_long_as_the_stream_does() {
    let mut state = State::default();
    assert!(!pouring_visually(&state), "an idle furnace shows no pour");

    state.phase = Phase::Pouring;
    assert!(pouring_visually(&state), "the tap is open");

    state.phase = Phase::Setting;
    state.liquid = Liquid {
        head: 0.6,
        tail: 0.2,
        ..Liquid::default()
    };
    assert!(
        pouring_visually(&state),
        "early Setting still has metal in the air"
    );

    while state.liquid.head > state.liquid.tail {
        state.liquid.step(false, 0.0);
    }
    assert_eq!(state.liquid.head, 0.0, "fixture: the catch-up resets");
    assert!(
        !pouring_visually(&state),
        "the pour is over once the stream has subsided"
    );
}

fn burning(ticks: u32) -> Burner {
    Burner {
        remaining: ticks,
        max: ticks,
    }
}

fn stack(item: &str) -> Option<ItemStackData> {
    Some(ItemStackData {
        item: item.into(),
        count: 4,
        data: Vec::new(),
    })
}

fn one_melt_tick(state: &mut State, item: &str) {
    let casting = Casting::for_test(
        &[],
        &[
            ("petramond:raw_iron", None),
            ("petramond:raw_copper", None),
            ("forge:iron_plate", Some("petramond:raw_iron")),
            ("forge:iron_pickaxe_head", Some("petramond:raw_iron")),
        ],
    );
    let mut slots = vec![stack(item), None, None];
    ForgingFurnaceSpec::default().melt(state, &mut slots, &casting, &mut Caches::default());
}

#[test]
fn only_a_declared_metal_reaches_the_crucible() {
    let lit = || State {
        fire: burning(200),
        ..State::default()
    };

    let mut sand = lit();
    one_melt_tick(&mut sand, "petramond:sand");
    assert_eq!(sand.melt_progress, 0, "sand is not a metal, tag or no tag");

    let mut iron = lit();
    one_melt_tick(&mut iron, "petramond:raw_iron");
    assert_eq!(iron.melt_progress, 1);
}

#[test]
fn a_loaded_crucible_takes_its_own_metal_and_nothing_else() {
    let holding_iron = || State {
        fire: burning(200),
        units: 1,
        metal: "petramond:raw_iron".into(),
        ..State::default()
    };

    let mut foreign = holding_iron();
    one_melt_tick(&mut foreign, "petramond:raw_copper");
    assert_eq!(foreign.melt_progress, 0, "copper cannot join molten iron");

    let mut same = holding_iron();
    one_melt_tick(&mut same, "petramond:raw_iron");
    assert_eq!(same.melt_progress, 1);

    let mut remelt = holding_iron();
    one_melt_tick(&mut remelt, "forge:iron_plate");
    assert_eq!(remelt.melt_progress, 1, "a plate melts back into its metal");
}

#[test]
fn a_cast_head_remelts_into_its_own_metal_only() {
    let mut same = State {
        fire: burning(200),
        units: 1,
        metal: "petramond:raw_iron".into(),
        ..State::default()
    };
    one_melt_tick(&mut same, "forge:iron_pickaxe_head");
    assert_eq!(same.melt_progress, 1, "a head melts back into its metal");

    let mut foreign = State {
        fire: burning(200),
        units: 1,
        metal: "petramond:raw_copper".into(),
        ..State::default()
    };
    one_melt_tick(&mut foreign, "forge:iron_pickaxe_head");
    assert_eq!(
        foreign.melt_progress, 0,
        "a head cannot join a different metal"
    );
}

#[test]
fn automatic_pour_waits_again_after_a_mould_swap_or_lost_readiness() {
    let mut state = State::default();
    assert!(!auto_ready(&mut state, true, Some("test:a"), 3));
    assert!(!auto_ready(&mut state, true, Some("test:a"), 3));
    assert!(!auto_ready(&mut state, true, Some("test:b"), 3));
    assert!(!auto_ready(&mut state, true, Some("test:b"), 3));
    assert!(!auto_ready(&mut state, false, Some("test:b"), 3));
    for _ in 0..2 {
        assert!(!auto_ready(&mut state, true, Some("test:b"), 3));
    }
    assert!(auto_ready(&mut state, true, Some("test:b"), 3));
    for _ in 0..10 {
        assert!(!auto_ready(&mut state, true, None, 3));
    }
}
