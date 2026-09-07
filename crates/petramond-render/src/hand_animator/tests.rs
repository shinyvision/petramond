use super::*;
use petramond_world::item::ItemType;

/// The held pose eases toward its published target and back to the
/// authored hold when it clears — the smoothing that turns a REPLICATED
/// publisher's 20 Hz steps into a glide, and the reason a released guard
/// drops rather than snaps.
#[test]
fn pose_eases_toward_its_target_and_back_to_the_authored_hold() {
    let mut anim = HeldItemAnimator::default();
    let dt = 1.0 / 60.0;
    let mut guard = HeldPose::default();
    guard.first_person.translation = [0.0, -7.0, 2.0];
    guard.third_person.rotation = [-40.0, 0.0, 0.0];
    let frame = |target: Option<HeldPose>| HeldItemFrame {
        item: None,
        display: None,
        variant: petramond_world::item::VariantId::NONE,
        block_state: Default::default(),
        mining: false,
        broke_block: false,
        placed: false,
        swung: false,
        eating: None,
        pose_target: target,
        swing_claim: false,
        jab_claim: false,
        bob: [0.0, 0.0],
        motion_offset: [0.0; 3],
        dt,
    };

    for _ in 0..120 {
        let view = anim.update(frame(Some(guard)));
        assert_eq!(view.swing, 0.0, "a pose is not a swing");
        if view.pose.first_person.translation[1] < -6.9 {
            break;
        }
    }
    assert!(anim.pose.first_person.translation[1] < -6.9);
    assert!((anim.pose.third_person.rotation[0] + 40.0).abs() < 2.0);
    assert_eq!(
        anim.pose.first_person.scale, [1.0; 3],
        "easing must never disturb the channels a mod cannot set"
    );

    for _ in 0..120 {
        let p = anim.update(frame(None)).pose;
        if p.first_person.translation[1] > -0.05 && p.third_person.rotation[0].abs() < 0.05 {
            break;
        }
    }
    assert!(anim.pose.first_person.translation[1] > -0.05);
    assert!(anim.pose.third_person.rotation[0].abs() < 0.05);
}

/// A pose belongs to the ITEM it was eased for. Swapping the hotbar must
/// draw the incoming item at its own authored hold, not glide it out of
/// the outgoing item's offset.
///
/// The bug this pins was invisible in the case you would test first —
/// switching away from a RAISED guard, whose offset is already neutral —
/// and obvious only when switching away from a LOWERED one, which is why
/// the reset is keyed on the item rather than on the target going `None`.
#[test]
fn a_new_item_starts_at_its_own_hold_instead_of_the_last_ones() {
    let mut anim = HeldItemAnimator::default();
    let dt = 1.0 / 60.0;
    let mut lowered = HeldPose::default();
    lowered.first_person.translation = [0.0, -6.0, 0.0];
    let frame = |item, target| HeldItemFrame {
        item,
        display: None,
        variant: petramond_world::item::VariantId::NONE,
        block_state: Default::default(),
        mining: false,
        broke_block: false,
        placed: false,
        swung: false,
        eating: None,
        pose_target: target,
        swing_claim: false,
        jab_claim: false,
        bob: [0.0, 0.0],
        motion_offset: [0.0; 3],
        dt,
    };

    // Settle the first item at its lowered offset.
    for _ in 0..60 {
        anim.update(frame(Some(ItemType::Stone), Some(lowered)));
    }
    assert!(anim.pose.first_person.translation[1] < -5.0);

    // The next item is drawn at ITS hold on the very first frame.
    let view = anim.update(frame(Some(ItemType::Dirt), None));
    assert_eq!(
        view.pose.first_person.translation, [0.0; 3],
        "a swapped-in item must not wear the last item's offset"
    );

    // An empty hand counts as a change too — and so does picking the
    // first item back up.
    anim.update(frame(Some(ItemType::Stone), Some(lowered)));
    let view = anim.update(frame(None, None));
    assert_eq!(view.pose.first_person.translation, [0.0; 3]);
}

#[test]
fn animator_completes_active_swing_when_mining_stops() {
    let mut anim = HeldItemAnimator {
        swing_t: 0.5,
        ..HeldItemAnimator::default()
    };
    let view = anim.update(HeldItemFrame {
        item: None,
        display: None,
        variant: petramond_world::item::VariantId::NONE,
        block_state: Default::default(),
        mining: false,
        broke_block: false,
        placed: false,
        swung: false,
        eating: None,
        pose_target: None,
        swing_claim: false,
        jab_claim: false,
        bob: [0.0, 0.0],
        motion_offset: [0.0; 3],
        dt: 1.0 / 60.0,
    });
    assert!(
        view.swing > 0.5,
        "stopping mining should finish the swing forward, not rewind it"
    );

    let settled = anim.update(HeldItemFrame {
        item: None,
        display: None,
        variant: petramond_world::item::VariantId::NONE,
        block_state: Default::default(),
        mining: false,
        broke_block: false,
        placed: false,
        swung: false,
        eating: None,
        pose_target: None,
        swing_claim: false,
        jab_claim: false,
        bob: [0.0, 0.0],
        motion_offset: [0.0; 3],
        dt: 0.5 * vanilla_swing().one_shot_seconds,
    });
    assert_eq!(settled.swing, 0.0);
}

#[test]
fn animator_plays_one_swing_for_instant_break_from_rest() {
    let mut anim = HeldItemAnimator::default();

    let started = anim.update(HeldItemFrame {
        item: None,
        display: None,
        variant: petramond_world::item::VariantId::NONE,
        block_state: Default::default(),
        mining: false,
        broke_block: true,
        placed: false,
        swung: false,
        eating: None,
        pose_target: None,
        swing_claim: false,
        jab_claim: false,
        bob: [0.0, 0.0],
        motion_offset: [0.0; 3],
        dt: 0.0,
    });
    assert_eq!(
        started.swing, 0.0,
        "zero-dt break event can begin at the rest pose"
    );

    let moving = anim.update(HeldItemFrame {
        item: None,
        display: None,
        variant: petramond_world::item::VariantId::NONE,
        block_state: Default::default(),
        mining: false,
        broke_block: false,
        placed: false,
        swung: false,
        eating: None,
        pose_target: None,
        swing_claim: false,
        jab_claim: false,
        bob: [0.0, 0.0],
        motion_offset: [0.0; 3],
        dt: 1.0 / 60.0,
    });
    assert!(
        moving.swing > 0.0,
        "instant block break should keep animating after the break frame"
    );

    let settled = anim.update(HeldItemFrame {
        item: None,
        display: None,
        variant: petramond_world::item::VariantId::NONE,
        block_state: Default::default(),
        mining: false,
        broke_block: false,
        placed: false,
        swung: false,
        eating: None,
        pose_target: None,
        swing_claim: false,
        jab_claim: false,
        bob: [0.0, 0.0],
        motion_offset: [0.0; 3],
        dt: vanilla_swing().one_shot_seconds,
    });
    assert_eq!(settled.swing, 0.0);
}

#[test]
fn animator_plays_one_full_swing_for_an_attack() {
    let mut anim = HeldItemAnimator::default();
    let started = anim.update(HeldItemFrame {
        item: None,
        display: None,
        variant: petramond_world::item::VariantId::NONE,
        block_state: Default::default(),
        mining: false,
        broke_block: false,
        placed: false,
        swung: true,
        eating: None,
        pose_target: None,
        swing_claim: false,
        jab_claim: false,
        bob: [0.0, 0.0],
        motion_offset: [0.0; 3],
        dt: 1.0 / 60.0,
    });
    assert!(started.swing > 0.0, "an attack begins a swing");
    assert_eq!(
        started.swing_scale, 1.0,
        "an attack swings at full strength"
    );

    // It carries through and settles like any one-shot swing.
    let settled = anim.update(HeldItemFrame {
        item: None,
        display: None,
        variant: petramond_world::item::VariantId::NONE,
        block_state: Default::default(),
        mining: false,
        broke_block: false,
        placed: false,
        swung: false,
        eating: None,
        pose_target: None,
        swing_claim: false,
        jab_claim: false,
        bob: [0.0, 0.0],
        motion_offset: [0.0; 3],
        dt: vanilla_swing().one_shot_seconds,
    });
    assert_eq!(settled.swing, 0.0, "the attack swing completes");
}

#[test]
fn animator_turns_place_event_into_one_softer_swing() {
    let mut anim = HeldItemAnimator::default();
    let placed = anim.update(HeldItemFrame {
        item: Some(ItemType::Dirt),
        display: None,
        variant: petramond_world::item::VariantId::NONE,
        block_state: Default::default(),
        mining: false,
        broke_block: false,
        placed: true,
        swung: false,
        eating: None,
        pose_target: None,
        swing_claim: false,
        jab_claim: false,
        bob: [0.0, 0.0],
        motion_offset: [0.0; 3],
        dt: 1.0 / 60.0,
    });
    // A place starts a one-shot swing at the reduced place amplitude (softer
    // than a mining punch — the `PLACE_SWING_SCALE < 1.0` guard is a static
    // assertion at the constant's definition).
    assert!(placed.swing > 0.0, "place should begin a swing");
    assert_eq!(placed.swing_scale, PLACE_SWING_SCALE);

    // ...which completes and returns to rest within one swing period.
    let settled = anim.update(HeldItemFrame {
        item: Some(ItemType::Dirt),
        display: None,
        variant: petramond_world::item::VariantId::NONE,
        block_state: Default::default(),
        mining: false,
        broke_block: false,
        placed: false,
        swung: false,
        eating: None,
        pose_target: None,
        swing_claim: false,
        jab_claim: false,
        bob: [0.0, 0.0],
        motion_offset: [0.0; 3],
        dt: vanilla_swing().one_shot_seconds,
    });
    assert_eq!(settled.swing, 0.0);
}

#[test]
fn animator_place_swing_carries_onto_emptied_hand() {
    // Placing the last block empties the hand the same frame (item -> None).
    // The swing must still fire so the bare arm animates the placement.
    let mut anim = HeldItemAnimator::default();
    let view = anim.update(HeldItemFrame {
        item: None,
        display: None,
        variant: petramond_world::item::VariantId::NONE,
        block_state: Default::default(),
        mining: false,
        broke_block: false,
        placed: true,
        swung: false,
        eating: None,
        pose_target: None,
        swing_claim: false,
        jab_claim: false,
        bob: [0.0, 0.0],
        motion_offset: [0.0; 3],
        dt: 1.0 / 60.0,
    });
    assert_eq!(
        view.item, None,
        "hand is empty after placing the last block"
    );
    assert!(
        view.swing > 0.0,
        "the emptied hand still plays the place swing"
    );
    assert_eq!(view.swing_scale, PLACE_SWING_SCALE);
}

#[test]
fn animator_eat_rides_its_own_channels_not_the_swing() {
    let mut anim = HeldItemAnimator::default();
    let eat_frame = |dt: f32, progress: f32| HeldItemFrame {
        item: Some(ItemType::Stone),
        display: None,
        variant: petramond_world::item::VariantId::NONE,
        block_state: Default::default(),
        mining: false,
        broke_block: false,
        placed: false,
        swung: false,
        eating: Some(progress),
        pose_target: None,
        swing_claim: false,
        jab_claim: false,
        bob: [0.0, 0.0],
        motion_offset: [0.0; 3],
        dt,
    };

    // The raise eases in (not a snap), never plays the punch, and settles
    // at the FULL mouth spot regardless of progress — the progress drives
    // only the toward-the-camera approach, not the screen carry.
    let first = anim.update(eat_frame(1.0 / 60.0, 0.0));
    assert!(first.eat > 0.0 && first.eat < 1.0, "carry eases in");
    assert_eq!(first.swing, 0.0, "eating never plays the punch channel");
    let raised = anim.update(eat_frame(1.0, 0.0));
    assert_eq!(raised.eat, 1.0, "the raise settles at the mouth spot");
    assert!(
        raised.eat_near < 0.05,
        "no camera approach yet at progress 0, got {}",
        raised.eat_near
    );

    // The approach tracks the sim's progress monotonically, reaching the
    // camera-nearest seat only at the end of the eat.
    let mid = anim.update(eat_frame(1.0, 0.5));
    assert!(
        (mid.eat_near - 0.5).abs() < 1e-3,
        "half-eaten food is halfway through its approach, got {}",
        mid.eat_near
    );
    let done = anim.update(eat_frame(1.0, 1.0));
    assert!(
        (done.eat_near - 1.0).abs() < 1e-3,
        "the last bite happens nearest the camera, got {}",
        done.eat_near
    );

    // The nibble oscillates sign over a bite period.
    let a = anim.update(eat_frame(0.5 / EAT_CHEW_HZ, 1.0)).eat_bob;
    let b = anim.update(eat_frame(0.5 / EAT_CHEW_HZ, 1.0)).eat_bob;
    assert!(
        a.signum() != b.signum() || (a - b).abs() > 0.5,
        "the bite rhythm oscillates: {a} vs {b}"
    );

    // Ending the eat eases the carry back out to rest.
    let releasing = anim.update(HeldItemFrame {
        eating: None,
        ..eat_frame(1.0 / 60.0, 1.0)
    });
    assert!(releasing.eat < 1.0, "release starts easing out");
    let rested = anim.update(HeldItemFrame {
        eating: None,
        ..eat_frame(1.0, 1.0)
    });
    assert_eq!(rested.eat, 0.0, "the carry returns fully to rest");
}

#[test]
fn animator_mining_punch_is_full_strength() {
    let mut anim = HeldItemAnimator::default();
    let view = anim.update(HeldItemFrame {
        item: None,
        display: None,
        variant: petramond_world::item::VariantId::NONE,
        block_state: Default::default(),
        mining: true,
        broke_block: false,
        placed: false,
        swung: false,
        eating: None,
        pose_target: None,
        swing_claim: false,
        jab_claim: false,
        bob: [0.0, 0.0],
        motion_offset: [0.0; 3],
        dt: 1.0 / 60.0,
    });
    assert_eq!(view.swing_scale, 1.0, "mining is the full-strength punch");
}

/// A claimed hand swings NOTHING of the engine's own — the mining level
/// and the break/attack triggers are the claimant's to animate — but the
/// place/use JAB is deliberately outside the claim: it still pops, at
/// its reduced amplitude, so a claimed hand placing a block reads
/// exactly like an unclaimed one. The POSE seam keeps easing under the
/// claim throughout, and releasing the claim restores the full machine.
#[test]
fn a_claimed_hand_swings_nothing_but_still_jabs_and_poses() {
    let mut anim = HeldItemAnimator::default();
    let dt = 1.0 / 60.0;
    let mut guard = HeldPose::default();
    guard.first_person.translation = [0.0, -4.0, 0.0];
    let frame = |claim: bool, placed: bool, pose: Option<HeldPose>| HeldItemFrame {
        item: None,
        display: None,
        variant: petramond_world::item::VariantId::NONE,
        block_state: Default::default(),
        mining: true,
        broke_block: true,
        placed,
        swung: true,
        eating: None,
        pose_target: pose,
        swing_claim: claim,
        jab_claim: false,
        bob: [0.0, 0.0],
        motion_offset: [0.0; 3],
        dt,
    };

    // The mining level and the break/attack triggers do nothing.
    for _ in 0..20 {
        let view = anim.update(frame(true, false, Some(guard)));
        assert_eq!(view.swing, 0.0, "a claimed hand never plays the swing");
        // The pose channel still chases its target under the claim.
        assert!(view.pose.first_person.translation[1] <= 0.0);
    }
    assert!(
        anim.pose.first_person.translation[1] < -3.0,
        "the eased pose moved"
    );

    // A placement still jabs — reduced amplitude, exactly the unclaimed
    // read — and finishes home while the claim stands.
    let view = anim.update(frame(true, true, Some(guard)));
    assert!(view.swing > 0.0, "the place jab is not part of the claim");
    assert_eq!(view.swing_scale, PLACE_SWING_SCALE);
    for _ in 0..((vanilla_swing().one_shot_seconds / dt) as usize + 2) {
        anim.update(frame(true, false, Some(guard)));
    }
    assert_eq!(anim.swing_t, 0.0, "the jab finished home under the claim");

    // Releasing the claim hands the full machine back.
    let view = anim.update(frame(false, false, None));
    assert!(view.swing > 0.0, "releasing the claim restores the punch");
}

/// A claimed JAB starts no engine jab — `placed` is the claimant's to
/// animate — while the SWING machine keeps running untouched: the two
/// motions are separately owned, so a jab-only claimant still gets the
/// full vanilla mining loop and punches.
#[test]
fn a_claimed_jab_never_starts_and_the_swing_machine_keeps_running() {
    let mut anim = HeldItemAnimator::default();
    let frame = |placed: bool, mining: bool| HeldItemFrame {
        item: None,
        display: None,
        variant: petramond_world::item::VariantId::NONE,
        block_state: Default::default(),
        mining,
        broke_block: false,
        placed,
        swung: false,
        eating: None,
        pose_target: None,
        swing_claim: false,
        jab_claim: true,
        bob: [0.0, 0.0],
        motion_offset: [0.0; 3],
        dt: 1.0 / 60.0,
    };

    let view = anim.update(frame(true, false));
    assert_eq!(view.swing, 0.0, "a claimed jab never starts");

    let view = anim.update(frame(false, true));
    assert!(view.swing > 0.0, "the swing motion stays the engine's");
    assert_eq!(view.swing_scale, 1.0, "at mining strength, not a jab");
}

/// The hand CHASES the camera's sway; it does not wear it. Pinned because
/// the obvious "simplification" — assigning `frame.bob` straight through —
/// puts the item in rigid lockstep with the camera, which is precisely the
/// look view bob exists to avoid, and nothing else in the frame would show
/// it.
#[test]
fn the_hand_trails_the_cameras_sway_instead_of_matching_it() {
    let mut anim = HeldItemAnimator::default();
    let dt = 1.0 / 60.0;
    let frame = |bob: [f32; 2]| HeldItemFrame {
        item: Some(ItemType::Stone),
        display: None,
        variant: petramond_world::item::VariantId::NONE,
        block_state: Default::default(),
        mining: false,
        broke_block: false,
        placed: false,
        swung: false,
        eating: None,
        pose_target: None,
        swing_claim: false,
        jab_claim: false,
        bob,
        motion_offset: [0.0; 3],
        dt,
    };

    // A step to full sway is approached, never taken in one frame.
    let first = anim.update(frame([1.0, 0.0]));
    assert!(
        first.bob[0] > 0.0 && first.bob[0] < HAND_BOB_SWAY * 0.5,
        "one frame must only start the chase: {}",
        first.bob[0]
    );

    // Under the real thing — a sine at a walk's sway rate — the hand runs
    // BEHIND and SHORTER than the camera. Both matter: a pure phase offset
    // would keep full amplitude, and pure damping would keep the timing.
    let mut anim = HeldItemAnimator::default();
    let omega = 9.5_f32; // rad/s, a walk's lateral sway
    let (mut camera, mut hand) = (Vec::new(), Vec::new());
    for i in 0..600 {
        let t = i as f32 * dt;
        let c = (omega * t).sin();
        hand.push(anim.update(frame([c, 0.0])).bob[0] / HAND_BOB_SWAY);
        camera.push(c);
    }
    // Measure over the last few cycles, past the start transient.
    let tail = 300;
    let peak = |v: &[f32]| v[tail..].iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(
        peak(&hand) < peak(&camera) * 0.9,
        "the hand should swing shorter: {} vs {}",
        peak(&hand),
        peak(&camera)
    );
    let rising_zero = |v: &[f32]| {
        v.windows(2)
            .enumerate()
            .skip(tail)
            .find(|(_, w)| w[0] < 0.0 && w[1] >= 0.0)
            .map(|(i, _)| i)
            .expect("a rising crossing in the tail")
    };
    assert!(
        rising_zero(&hand) > rising_zero(&camera),
        "the hand should cross LATER: {} vs {}",
        rising_zero(&hand),
        rising_zero(&camera)
    );
}
