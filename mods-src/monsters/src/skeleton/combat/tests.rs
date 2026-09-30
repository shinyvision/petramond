use std::sync::{Arc, Mutex};

use super::super::aim::Flight;
use super::super::kit::{Bow, Melee, Ranged, Swing};
use super::*;

fn mob(id: u64, pos: [f64; 3]) -> MobSnapshot {
    MobSnapshot {
        id,
        pos,
        kind: MobId(1),
        index: 0,
        health: 20.0,
        yaw: 0.0,
        pitch: 0.0,
        roll: 0.0,
        vel: [0.0; 3],
        on_ground: true,
        moving: false,
        half_width: 0.3,
        half_length: 0.3,
        height: 1.8,
        entombed: false,
        conditions: vec![],
        target: None,
    }
}

#[derive(Default)]
struct HostState {
    foe: Option<MobSnapshot>,
    safe: bool,
    shots: usize,
    hits: Vec<(u64, Option<EntityRef>)>,
}

fn turn<'a>(now: u64, me: &'a MobSnapshot, kit: &'a Kit) -> Turn<'a> {
    Turn {
        now,
        me,
        kit,
        hold: false,
        velocity: None,
        retreating: false,
        foe: None,
        plays: vec![],
    }
}

#[test]
fn the_brain_lock_drives_mob_hits_mobile_archery_and_cornered_melee() {
    let host = Arc::new(Mutex::new(HostState::default()));
    let shared = host.clone();
    let _guard = mod_sdk::testing::install_host(move |call| {
        let mut state = shared.lock().unwrap();
        match call {
            HostCall::Block(BlockCall::Raycast { .. }) => HostRet::Raycast(None),
            HostCall::Core(CoreCall::RngU64 { .. }) => HostRet::U64(7),
            HostCall::Entity(EntityCall::MobInfo { mob_id }) => {
                HostRet::Mob(state.foe.clone().filter(|m| m.id == *mob_id))
            }
            HostCall::Entity(EntityCall::MobsInRadius { .. }) => {
                HostRet::Mobs(state.foe.clone().into_iter().collect())
            }
            HostCall::Entity(EntityCall::MobWalkProbe { offsets, .. }) => {
                HostRet::Bools(vec![state.safe; offsets.len()])
            }
            HostCall::Entity(EntityCall::MobAnimSet { .. }) => HostRet::Bool(true),
            HostCall::Entity(EntityCall::MobAnimState { .. }) => HostRet::MobAnimState(None),
            HostCall::Entity(EntityCall::LaunchItem { .. }) => {
                state.shots += 1;
                HostRet::U64(42)
            }
            HostCall::Entity(EntityCall::DamageMob {
                mob_id, attacker, ..
            }) => {
                state.hits.push((*mob_id, *attacker));
                HostRet::Unit
            }
            _ => panic!("unexpected call {call:?}"),
        }
    });
    let melee = Melee {
        damage: 2.0,
        knockback: 0.0,
        reach: 1.3,
        windup: 2,
        cooldown: 10,
        clip: "punch".into(),
    };
    let mut kit = Kit {
        swing: Some(Swing { hand: None, melee }),
        ..Kit::default()
    };
    let mut me = mob(1, [0.0; 3]);
    let mut body = Body {
        loadout: None,
        post: None,
        watch: false,
        presence: Default::default(),
        fight: Default::default(),
    };
    let mut shoves = vec![];
    host.lock().unwrap().foe = Some(mob(9, [0.0, 0.0, -1.5]));
    // No lock means no strike, even with a live body beside us.
    turn(100, &me, &kit).run(&mut body, &[], &mut shoves);
    assert_eq!(body.fight.act, Act::Idle);
    me.target = Some(EntityRef::Mob(9));
    turn(101, &me, &kit).run(&mut body, &[], &mut shoves);
    assert!(matches!(body.fight.act, Act::Swing { .. }));
    turn(103, &me, &kit).run(&mut body, &[], &mut shoves);
    assert_eq!(host.lock().unwrap().hits, [(9, Some(EntityRef::Mob(1)))]);

    kit.bow = Some(Bow {
        hand: Hand::Off,
        flight: Flight::default(),
        ranged: Ranged {
            ammo: "x:arrow".into(),
            pull: vec![],
            draw: 3,
            speed: 28.0,
            spread: 0.0,
            min_range: 3.0,
            max_range: 24.0,
            rest: 6,
            clip_draw: "draw".into(),
            clip_loose: "loose".into(),
        },
    });
    body.fight = Fight::default();
    body.watch = true;
    {
        let mut state = host.lock().unwrap();
        state.safe = true;
        state.foe = Some(mob(9, [0.0, 0.0, -4.0]));
    }
    let mut drawing = turn(200, &me, &kit);
    drawing.run(&mut body, &[], &mut shoves);
    assert!(
        drawing.velocity.unwrap()[1] > 0.0,
        "backpedal while aiming forward"
    );
    assert!(matches!(body.fight.act, Act::Draw { .. }));
    let mut release = turn(204, &me, &kit);
    release.run(&mut body, &[], &mut shoves);
    assert!(release.velocity.is_some(), "release while still retreating");
    assert_eq!(
        host.lock().unwrap().shots,
        1,
        "the intended mob must not block its own shot"
    );

    // An already drawn bow must let down when a nearby foe corners the archer.
    body.fight.act = Act::Draw {
        release: 225,
        give_up: 260,
    };
    body.fight.ready_at = 220;
    {
        let mut state = host.lock().unwrap();
        state.safe = false;
        state.foe = Some(mob(9, [0.0, 0.0, -1.0]));
    }
    let mut cornered = turn(220, &me, &kit);
    cornered.run(&mut body, &[], &mut shoves);
    assert!(cornered.velocity.is_none());
    assert!(cornered.hold);
    assert!(matches!(body.fight.act, Act::Swing { .. }));
    turn(222, &me, &kit).run(&mut body, &[], &mut shoves);
    assert_eq!(host.lock().unwrap().hits.len(), 2);

    me.target = None;
    turn(223, &me, &kit).run(&mut body, &[], &mut shoves);
    assert_eq!(
        body.fight.act,
        Act::Idle,
        "a cleared lock cancels the old strike"
    );
}
