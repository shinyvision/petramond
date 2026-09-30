use super::*;
use crate::charge::Draw;

fn row() -> Row {
    Row {
        id: ItemId(19),
        name: "test:boomerang".into(),
        draw: Draw {
            full_ticks: 10,
            strain_ticks: 5,
            speed_scale: 0.7,
            launch_speed: [5.0, 20.0],
        },
        outbound_ticks: [3, 9],
        damage_weak: [1.0, 2.0],
        damage_full: [4.0, 6.0],
        return_speed: 25.0,
        curve: 0.4,
        turn_acceleration: 80.0,
        motion: rows::Motion {
            first_person: ["test:draw".into(), "test:throw".into()],
            body: ["test:draw_body".into(), "test:throw_body".into()],
            length: 0.5,
            fp_length: 0.5,
            fp_release: 0.18,
        },
    }
}

fn actor() -> PlayerSnapshot {
    PlayerSnapshot {
        id: Some(PlayerId(0)),
        pos: [0.0; 3],
        vel: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        health: 20,
        on_ground: true,
        spectator: false,
        sneak: false,
        use_held: true,
        holds_use: true,
        held: Some(row().id),
        off_held: None,
        held_count: 1,
        pose_anchor: None,
        swing: Default::default(),
        half_width: 0.3,
        height: 1.8,
        eye_height: 1.62,
        entombed: false,
        conditions: vec![],
    }
}

#[test]
fn charge_changes_range_and_damage_and_the_curve_returns_through_the_launch_site() {
    let home = [0.0, 1.26, 0.0];
    let mut weak = Flight::new(PlayerId(0), row(), [0.0, 0.0, 1.0], home, 1);
    let mut full = Flight::new(
        PlayerId(0),
        row(),
        [0.0, 0.0, 1.0],
        home,
        row().draw.full_ticks,
    );
    assert!(full.outbound_velocity()[2] > weak.outbound_velocity()[2]);
    assert!(full.damage()[0] > weak.damage()[1]);
    let mut pos = [-0.35, 1.52, 0.0];
    let mut previous = full.outbound_velocity();
    let mut side = 0.0f64;
    let mut returned = false;
    for _ in 0..200 {
        let Some(vel) = full.step(pos) else {
            break;
        };
        let dot = (0..3).map(|i| previous[i] * vel[i]).sum::<f32>();
        assert!(
            dot > 0.0,
            "the unimpeded curve never reverses in one step: {previous:?} -> {vel:?}"
        );
        let next = strike::offset_by(pos, vel.map(|v| v * crate::body::TICK_SECONDS));
        if vel[2] < 0.0 && pos[2] >= home[2] && next[2] <= home[2] {
            let t = ((pos[2] - home[2]) / (pos[2] - next[2])) as f32;
            let crossing = strike::offset_by(pos, vel.map(|v| v * crate::body::TICK_SECONDS * t));
            assert!(
                (crossing[0] - home[0]).abs() < 0.15 && (crossing[1] - home[1]).abs() < 0.15,
                "returns through the captured home: {crossing:?}"
            );
            returned = true;
        }
        side = side.max(pos[0]);
        previous = vel;
        pos = next;
    }
    assert!(
        side > 0.5 && returned,
        "a completed return curve: {side} {returned}"
    );
    assert!(
        full.step(pos).is_none(),
        "an uncaught miss eventually drops"
    );
    let weak_turn = (0..row().outbound_ticks[0])
        .map(|_| weak.step([0.0, 1.26, 1.0]).unwrap())
        .last()
        .unwrap();
    assert!(weak_turn[0] > 0.0, "a short charge turns earlier");
    assert!(weak.strike(EntityRef::Mob(5)));
    assert!(
        !weak.strike(EntityRef::Mob(5)),
        "one hit per body across both legs"
    );
}

#[test]
fn a_body_hit_reverses_immediately_and_does_not_retarget_a_moving_owner() {
    let mut flight = Flight::new(PlayerId(0), row(), [0.0, 0.0, 1.0], [0.0, 1.26, 0.0], 10);
    assert_eq!(
        flight.reverse([0.0, 0.0, 20.0]),
        [0.0, 0.0, -row().return_speed]
    );
    let vel = flight.step([0.0, 1.26, 4.0]).unwrap();
    assert_eq!(vel, [0.0, 0.0, -row().return_speed]);
}

#[test]
fn an_uncaught_boomerang_drops_sixty_ticks_after_its_expected_catch() {
    for charge in [1, 10] {
        for body_hit in [false, true] {
            let row = row();
            let mut flight =
                Flight::new(PlayerId(0), row.clone(), [0.0, 0.0, 1.0], [0.0; 3], charge);
            let item = Rc::new(RefCell::new(ItemEntityData {
                id: 42,
                stack: ItemStackData {
                    item: row.name.clone(),
                    count: 1,
                    data: vec![("test:mark".into(), b"kept".to_vec())],
                },
                owner: Some(EntityRef::Player(PlayerId(0))),
                pos: [-0.35, 0.26, 0.0],
                vel: flight.outbound_velocity(),
                motion: ItemMotion::Flight,
            }));
            let live = item.clone();
            let _host = testing::install_host(move |call| match call {
                HostCall::Entity(EntityCall::ItemEntity { entity }) => {
                    assert_eq!(*entity, 42);
                    HostRet::ItemEntity(Some(Box::new(live.borrow().clone())))
                }
                HostCall::ItemMotion(ItemMotionCall::SteerItem { entity, vel }) => {
                    assert_eq!(*entity, 42);
                    let mut item = live.borrow_mut();
                    if let Some(vel) = vel {
                        item.vel = *vel;
                    } else {
                        item.motion = ItemMotion::Loose;
                    }
                    HostRet::Bool(true)
                }
                _ => panic!("unexpected {call:?}"),
            });
            if body_hit {
                flight.reverse(item.borrow().vel);
                item.borrow_mut().pos = [0.0, 0.0, 4.0];
            }
            let boomerangs = Boomerangs {
                rows: vec![row],
                flights: RefCell::new([(42, flight)].into()),
            };
            let mut owner = actor();
            owner.pos = [100.0, 0.0, 0.0];
            let roster = [PlayerListEntry {
                id: PlayerId(0),
                state: owner,
            }];
            let stack = item.borrow().stack.clone();
            let mut catch_tick = None;
            let mut dropped = false;
            for tick in 1..=200 {
                boomerangs.step(&roster);
                let mut item = item.borrow_mut();
                if item.motion == ItemMotion::Loose {
                    assert_eq!(
                        tick,
                        catch_tick.expect("returns through the throw site") + 60
                    );
                    assert_eq!(item.stack, stack);
                    assert!(boomerangs.flights.borrow().is_empty());
                    dropped = true;
                    break;
                }
                let next =
                    strike::offset_by(item.pos, item.vel.map(|v| v * crate::body::TICK_SECONDS));
                if item.vel[2] < 0.0 && item.pos[2] >= 0.0 && next[2] <= 0.0 {
                    catch_tick = Some(tick);
                }
                item.pos = next;
            }
            assert!(dropped, "a miss in open space becomes a collectable item");
        }
    }
}

#[test]
fn release_spends_the_held_variant_once_and_changing_items_cancels_the_charge() {
    let stack = ItemStackData {
        item: row().name,
        count: 1,
        data: vec![("test:mark".into(), b"kept".to_vec())],
    };
    let seen = Rc::new(RefCell::new((0, 0, 0)));
    let calls = seen.clone();
    let _host = testing::install_host(move |call| match call {
        HostCall::Player(PlayerCall::PlayerHeld { .. }) => HostRet::HeldStack(Some(stack.clone())),
        HostCall::Player(PlayerCall::ConsumeHeldBy { item, count, .. }) => {
            assert_eq!(*item, row().id);
            assert_eq!(*count, 1);
            calls.borrow_mut().0 += 1;
            HostRet::Bool(true)
        }
        HostCall::Entity(EntityCall::LaunchItem { data, .. }) => {
            assert_eq!(data, &stack.data);
            calls.borrow_mut().1 += 1;
            HostRet::U64(42)
        }
        HostCall::ItemMotion(ItemMotionCall::SteerItem { .. }) => HostRet::Bool(true),
        HostCall::Block(BlockCall::Raycast { .. }) => HostRet::Raycast(None),
        HostCall::Entity(EntityCall::MobsInRadius { .. }) => HostRet::Mobs(vec![]),
        HostCall::Player(PlayerCall::Players) => HostRet::Players(vec![]),
        _ => panic!("unexpected {call:?}"),
    });
    let boomerangs = Rc::new(Boomerangs {
        rows: vec![row()],
        flights: RefCell::default(),
    });
    let rule = ThrowRule::new(boomerangs.clone());
    let mut clocks = BodyClocks::default();
    let state = actor();
    rule.step(&mut clocks, PlayerId(0), &state, true, 4.0, true);
    assert!(
        rule.claims(&Body {
            state: &state,
            clocks: &clocks,
            press: true
        })
        .holds_press
    );
    rule.step(&mut clocks, PlayerId(0), &state, false, 1.0, true);
    rule.step(&mut clocks, PlayerId(0), &state, false, 1.0, true);
    assert_eq!(
        seen.borrow().0,
        0,
        "the item stays held until the release marker"
    );
    for _ in 0..12 {
        rule.step(&mut clocks, PlayerId(0), &state, false, 1.0, true);
    }
    assert_eq!(seen.borrow().0, 1);
    assert_eq!(seen.borrow().1, 1);
    assert_eq!(boomerangs.flights.borrow().len(), 1);
    rule.step(&mut clocks, PlayerId(0), &state, true, 3.0, true);
    let mut empty = state;
    empty.held = None;
    rule.step(&mut clocks, PlayerId(0), &empty, false, 1.0, true);
    assert_eq!(seen.borrow().1, 1, "switching away does not throw");
}

#[test]
fn terrain_drops_both_legs_and_a_catch_restores_the_stack_once() {
    let stack = ItemStackData {
        item: row().name,
        count: 1,
        data: vec![("test:mark".into(), b"kept".to_vec())],
    };
    let caught = Rc::new(RefCell::new(0));
    let gifts = caught.clone();
    let taken = stack.clone();
    let _host = testing::install_host(move |call| match call {
        HostCall::ItemMotion(ItemMotionCall::TakeItemEntity { .. }) => {
            HostRet::ItemStack(Some(taken.clone()))
        }
        HostCall::Player(PlayerCall::GiveItemTo {
            item, count, data, ..
        }) => {
            assert_eq!(item, &taken.item);
            assert_eq!(*count, taken.count);
            assert_eq!(data, &taken.data);
            *gifts.borrow_mut() += 1;
            HostRet::Bool(true)
        }
        _ => panic!("unexpected {call:?}"),
    });
    let boomerangs = Boomerangs {
        rows: vec![row()],
        flights: RefCell::default(),
    };
    for returning in [false, true] {
        let mut flight = Flight::new(PlayerId(0), row(), [0.0, 0.0, 1.0], [0.0, 1.26, 0.0], 5);
        if returning {
            flight.reverse([0.0, 0.0, 20.0]);
        }
        boomerangs.flights.borrow_mut().insert(42, flight);
        let mut fate = ProjectileFate::Lodge;
        let block = ProjectileTarget::Block {
            pos: [0, 1, 4],
            face: [0, 0, -1],
        };
        assert!(boomerangs.on_hit(42, &block, [0.0; 3], [0.0; 3], &mut fate, |_, _| false));
        assert_eq!(fate, ProjectileFate::Drop);
        assert!(boomerangs.flights.borrow().is_empty());
    }
    boomerangs.flights.borrow_mut().insert(
        43,
        Flight::new(PlayerId(0), row(), [0.0, 0.0, 1.0], [0.0, 1.26, 0.0], 5),
    );
    let mut fate = ProjectileFate::Drop;
    let target = ProjectileTarget::Player(PlayerId(0));
    assert!(boomerangs.on_hit(43, &target, [0.0; 3], [0.0; 3], &mut fate, |_, _| false));
    assert_eq!(fate, ProjectileFate::Consume);
    assert!(!boomerangs.on_hit(43, &target, [0.0; 3], [0.0; 3], &mut fate, |_, _| false));
    assert_eq!(*caught.borrow(), 1);
}

#[test]
fn a_throw_keeps_its_follow_through_when_the_item_leaves_and_cancels_before_release() {
    let row = row();
    let mut clock = motion::Clock::default();
    clock.step(Some(&row), Some(row.id), true, true, 4.0);
    clock.step(Some(&row), Some(row.id), true, false, 1.0);
    assert!(matches!(clock.pose(), Some(Pose::Throw(_))));
    clock.step(Some(&row), None, true, false, 1.0);
    assert!(
        clock.pose().is_none(),
        "losing the weapon before release cancels"
    );
    clock.step(Some(&row), Some(row.id), true, true, 4.0);
    clock.step(Some(&row), Some(row.id), true, false, 1.0);
    let mut releases = 0;
    let mut held = Some(row.id);
    for _ in 0..40 {
        if clock
            .step(Some(&row), held, true, false, 1.0 / 3.0)
            .is_some()
        {
            releases += 1;
            held = None;
            assert!(
                throw_claims(Some(&row), &clock).plays.len() == 2,
                "both views finish with an empty hand"
            );
        }
    }
    assert_eq!(releases, 1);
    assert!(clock.pose().is_none());
    clock.step(Some(&row), Some(row.id), true, true, 4.0);
    clock.step(Some(&row), Some(row.id), true, false, 1.0);
    assert!(clock
        .step(Some(&row), Some(ItemId(99)), true, false, 10.0)
        .is_none());
    assert!(
        clock.pose().is_none(),
        "switching weapons during windup cancels"
    );
}

#[test]
fn throw_views_start_together_and_keep_their_authored_seconds() {
    for (lengths, release) in [([0.8, 0.5], 0.25), ([0.2, 0.7], 0.12)] {
        let mut row = row();
        [row.motion.fp_length, row.motion.length] = lengths;
        row.motion.fp_release = release;
        assert_eq!(row.motion.release_time(), release);
        for (view, length) in lengths.into_iter().enumerate() {
            assert_eq!(row.motion.throw_times(0.0)[view], Some(0.0));
            for step in 0..100 {
                let time = length * step as f32 / 100.0;
                let progress = row.motion.throw_times(time)[view].unwrap();
                assert!(
                    (progress * length - time).abs() < 1e-6,
                    "each view advances one authored second per real second"
                );
            }
            assert!(row.motion.throw_times(length)[view].is_none());
        }
        let progress = row.motion.throw_times(release)[0].unwrap();
        assert!((progress * lengths[0] - release).abs() < 1e-6);
        assert!(row
            .motion
            .throw_times(row.motion.throw_length())
            .iter()
            .all(Option::is_none));
    }
}

#[test]
fn releasing_charge_starts_both_throws_without_a_wait_for_the_body_marker() {
    let mut row = row();
    row.motion.fp_length = 0.2;
    row.motion.fp_release = 0.12;
    let mut clock = motion::Clock::default();
    clock.step(
        Some(&row),
        Some(row.id),
        true,
        true,
        row.draw.full_ticks as f32,
    );
    assert!(matches!(clock.pose(), Some(Pose::Draw(_))));
    clock.step(Some(&row), Some(row.id), true, false, 0.0);
    assert!(matches!(clock.pose(), Some(Pose::Throw(0.0))));
    let frame = 1.0 / 60.0;
    assert!(clock
        .step(
            Some(&row),
            Some(row.id),
            true,
            false,
            frame / crate::body::TICK_SECONDS
        )
        .is_none());
    let claims = throw_claims(Some(&row), &clock);
    assert_eq!(claims.plays.len(), 2);
    for (play, length) in claims
        .plays
        .iter()
        .zip([row.motion.fp_length, row.motion.length])
    {
        let AnimatorClock::Scrub(progress) = play.clock else {
            panic!("a throw scrubs its clip");
        };
        assert!(
            progress > 0.0,
            "the very first frame after release advances the throw"
        );
        assert!((progress * length - frame).abs() < 1e-6);
    }
    assert_eq!(
        clock.step(
            Some(&row),
            Some(row.id),
            true,
            false,
            (row.motion.release_time() - frame) / crate::body::TICK_SECONDS + 0.001
        ),
        Some(row.draw.full_ticks)
    );
}

#[test]
fn completed_throw_views_release_their_claims_while_the_other_view_finishes() {
    for (fp_length, fp_release, elapsed, remaining) in [
        (0.2, 0.12, 0.3, rig::PLAYER_BODY),
        (0.8, 0.25, 0.6, rig::PLAYER_FIRST_PERSON),
    ] {
        let mut row = row();
        row.motion.fp_length = fp_length;
        row.motion.fp_release = fp_release;
        let mut clock = motion::Clock::default();
        clock.step(Some(&row), Some(row.id), true, true, 4.0);
        clock.step(Some(&row), Some(row.id), true, false, 1.0);
        assert_eq!(
            clock.step(
                Some(&row),
                Some(row.id),
                true,
                false,
                elapsed / crate::body::TICK_SECONDS
            ),
            Some(4)
        );
        let claims = throw_claims(Some(&row), &clock);
        assert_eq!(claims.plays.len(), 1);
        assert_eq!(claims.plays[0].rig, remaining);
        assert!(claims
            .params
            .iter()
            .filter(|param| param.param != "main.item_visible")
            .all(|param| param.rig == remaining));
        assert_eq!(
            claims
                .params
                .iter()
                .filter(|param| param.param == "main.item_visible")
                .count(),
            2,
            "the released item stays hidden while inventory replication catches up"
        );
        assert!(
            clock.pose().is_some(),
            "the longer recovery still owns the throw"
        );
        assert!(
            clock
                .step(
                    Some(&row),
                    None,
                    true,
                    false,
                    row.motion.throw_length() / crate::body::TICK_SECONDS
                )
                .is_none(),
            "recovery never launches twice"
        );
        assert!(clock.pose().is_none());
    }
}

#[test]
fn short_and_pitched_throws_turn_smoothly_in_large_world_coordinates() {
    for ticks in [1, 5, 10] {
        for pitch in [-0.6f32, 0.0, 0.7] {
            let home = [30_000_000.0, 700.0, -30_000_000.0];
            let dir = [
                0.4 * pitch.cos(),
                pitch.sin(),
                (1.0f32 - 0.4 * 0.4).sqrt() * pitch.cos(),
            ];
            let mut flight = Flight::new(PlayerId(0), row(), dir, home, ticks);
            let mut pos = strike::offset_by(home, [-0.35, 0.26, 0.0]);
            let mut previous = flight.outbound_velocity();
            let mut near = f32::INFINITY;
            for _ in 0..200 {
                let Some(vel) = flight.step(pos) else {
                    break;
                };
                assert!(
                    (0..3).map(|i| previous[i] * vel[i]).sum::<f32>() > 0.0,
                    "smooth low-charge and pitched return"
                );
                let next = strike::offset_by(pos, vel.map(|v| v * crate::body::TICK_SECONDS));
                let relative = strike::relative(home, pos);
                let segment = vel.map(|v| v * crate::body::TICK_SECONDS);
                let t = ((0..3).map(|i| relative[i] * segment[i]).sum::<f32>()
                    / segment.iter().map(|v| v * v).sum::<f32>())
                .clamp(0.0, 1.0);
                if vel.iter().zip(dir).map(|(v, d)| v * d).sum::<f32>() < 0.0 {
                    near = near.min(
                        (0..3)
                            .map(|i| (relative[i] - segment[i] * t).powi(2))
                            .sum::<f32>()
                            .sqrt(),
                    );
                }
                pos = next;
                previous = vel;
            }
            assert!(near < 0.15, "fixed throw location reached: {near}");
        }
    }
}

#[test]
fn a_narrow_turn_brakes_before_bending_and_eases_back_to_return_speed() {
    let mut row = row();
    row.draw.launch_speed = [8.0, 24.0];
    row.outbound_ticks = [6, 20];
    row.return_speed = 28.0;
    row.curve = 0.18;
    row.turn_acceleration = 70.0;
    let mut flight = Flight::new(PlayerId(0), row.clone(), [0.0, 0.0, 1.0], [0.0; 3], 10);
    let mut pos = [0.0; 3];
    let mut previous = flight.outbound_velocity();
    let mut previous_angle = 0.0f32;
    let mut slowest = f32::INFINITY;
    let mut widest = 0.0f64;
    let mut farthest = 0.0f64;
    let mut braked_before_turn = false;
    let mut accelerated_on_return = false;
    let speed = |v: [f32; 3]| v.iter().map(|v| v * v).sum::<f32>().sqrt();
    while let Some(vel) = flight.step(pos) {
        let dot = (0..3).map(|i| previous[i] * vel[i]).sum::<f32>();
        let angle = (dot / (speed(previous) * speed(vel)))
            .clamp(-1.0, 1.0)
            .acos();
        assert!(angle < 30.0f32.to_radians(), "gradual turn: {angle}");
        assert!(
            (angle - previous_angle).abs() < 10.0f32.to_radians(),
            "the turning rate eases in and out: {previous_angle} -> {angle}"
        );
        assert!(
            (speed(vel) - speed(previous)).abs()
                < row.turn_acceleration * crate::body::TICK_SECONDS * 1.15,
            "speed changes gradually: {} -> {}",
            speed(previous),
            speed(vel)
        );
        braked_before_turn |= speed(vel) < 23.5 && vel[0].abs() < 0.01;
        accelerated_on_return |= vel[2] < 0.0 && speed(vel) > 27.0;
        slowest = slowest.min(speed(vel));
        pos = strike::offset_by(pos, vel.map(|v| v * crate::body::TICK_SECONDS));
        widest = widest.max(pos[0]);
        farthest = farthest.max(pos[2]);
        previous = vel;
        previous_angle = angle;
    }
    assert!(braked_before_turn, "braking starts on the outbound leg");
    assert!(slowest < 16.0 && accelerated_on_return);
    assert!(widest > 0.5 && widest < farthest * 0.25, "narrow loop");
}
