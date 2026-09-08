use super::*;

/// A crucible that fills up and then goes cold must still be relightable.
///
/// Relight used to sit BEHIND the "is there something to melt" gate, and a
/// full crucible can melt nothing — so coal did nothing, the metal
/// hardened ten seconds later, and two minutes of smelting was gone with
/// no way to get it back.
#[test]
fn a_full_cold_crucible_still_wants_fuel() {
    let full = State {
        units: CRUCIBLE_MAX,
        metal: "petramond:raw_iron".into(),
        burn_remaining: 0,
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

/// The one bit has to follow the FIRE and nothing else. It used to carry a
/// lever as well; the lever is a GUI widget now, and a mask that still set
/// its bit would light a cube the model no longer has — which draws
/// nothing and is therefore invisible until someone wonders why the forge
/// looks cold while it is burning.
#[test]
fn the_fire_is_the_only_thing_the_mask_stages() {
    let spec = ForgingFurnaceSpec::default();
    assert_eq!(
        spec.parts_mask(&State::default()),
        0,
        "a cold forge shows no fire"
    );
    let lit = State {
        burn_remaining: 40,
        ..State::default()
    };
    assert_eq!(spec.parts_mask(&lit), PART_COALS);
}

/// THE STRIP IS A FUNCTION OF THE MACHINE, NOT OF THE CLICK. The lever
/// walks 0..7 over twelve ticks and then stays down until the machine is
/// idle again — a frame that lagged the pour or survived it would draw a
/// handle the machine is not holding.
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

/// THE POUR ROW FOLLOWS THE STREAM, NOT THE PHASE. The row carries the
/// droplet emitter, and the phase outlives the visible stream by the
/// whole set — keyed on the phase alone, droplets and spatter run for
/// SET_TICKS over a basin the metal finished falling into long ago. The
/// row holds while the tap is open or metal is airborne, and drops the
/// moment the tail catches the head.
#[test]
fn the_pour_row_outlives_the_phase_exactly_as_long_as_the_stream_does() {
    let mut state = State::default();
    assert!(!pouring_visually(&state), "an idle furnace shows no pour");

    state.phase = Phase::Pouring;
    assert!(pouring_visually(&state), "the tap is open");

    // The phase ends; what is already falling keeps falling.
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

    // Untapped, the tail falls until it catches the head and both reset.
    while state.liquid.head > state.liquid.tail {
        state.liquid.step(false, 0.0);
    }
    assert_eq!(state.liquid.head, 0.0, "fixture: the catch-up resets");
    assert!(
        !pouring_visually(&state),
        "the pour is over once the stream has subsided"
    );
}

fn stack(item: &str) -> Option<ItemStackData> {
    Some(ItemStackData {
        item: item.into(),
        count: 4,
        data: Vec::new(),
    })
}

/// A lit furnace with `item` in its metal slot, stepped one tick.
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
    ForgingFurnaceSpec::default().melt(state, &mut slots, &casting);
}

/// THE TABLE IS STILL THE WHITELIST. The panel's metal slot now names the
/// same `forge:metal` data key this table is built from, so the two agree
/// by construction rather than by hand — but the slot is not the only way
/// into the container (a mod write, a document that failed to load), and
/// the crucible is where the decision has to hold. If this check ever
/// stops being consulted a forge full of sand quietly produces iron.
#[test]
fn only_a_declared_metal_reaches_the_crucible() {
    let lit = || State {
        burn_remaining: 200,
        ..State::default()
    };

    let mut sand = lit();
    one_melt_tick(&mut sand, "petramond:sand");
    assert_eq!(sand.melt_progress, 0, "sand is not a metal, tag or no tag");

    let mut iron = lit();
    one_melt_tick(&mut iron, "petramond:raw_iron");
    assert_eq!(iron.melt_progress, 1);
}

/// THE CRUCIBLE NEVER MIXES, and the comparison is against the CANONICAL
/// metal. Both halves are silent when broken: mixing hands the player an
/// item they cannot explain, and comparing the raw item instead means a
/// second plate never matches the iron the first one became — the machine
/// just ignores a full stack for no visible reason.
#[test]
fn a_loaded_crucible_takes_its_own_metal_and_nothing_else() {
    let holding_iron = || State {
        burn_remaining: 200,
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

    // A plate declares `melts_to: raw_iron`, so it IS the metal already in
    // there — keyed on the item name it would not be.
    let mut remelt = holding_iron();
    one_melt_tick(&mut remelt, "forge:iron_plate");
    assert_eq!(remelt.melt_progress, 1, "a plate melts back into its metal");
}

/// A CAST HEAD IS THE METAL IT WAS POURED FROM. Remelting one has to join
/// a crucible of that metal and nothing else — otherwise a mistake at the
/// casting table is a stack of heads the furnace silently ignores, or
/// worse, dissolves into the wrong metal.
#[test]
fn a_cast_head_remelts_into_its_own_metal_only() {
    let mut same = State {
        burn_remaining: 200,
        units: 1,
        metal: "petramond:raw_iron".into(),
        ..State::default()
    };
    one_melt_tick(&mut same, "forge:iron_pickaxe_head");
    assert_eq!(same.melt_progress, 1, "a head melts back into its metal");

    let mut foreign = State {
        burn_remaining: 200,
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
