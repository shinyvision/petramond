use mod_sdk::*;

use crate::skeleton::combat::{wanted, Act, Fight};
use crate::skeleton::kit::{Guard, Hand, Kit, Melee, Shield, Swing};
use crate::skeleton::presence::{Play, Presence};

fn names(ops: &[MobAnimOp]) -> Vec<String> {
    ops.iter()
        .map(|op| match op {
            MobAnimOp::Set { anim, active, .. } => {
                format!("{}{anim}", if *active { "+" } else { "-" })
            }
            MobAnimOp::Seek { anim, phase, .. } => format!("@{anim}:{phase}"),
            MobAnimOp::Rate { anim, .. } => format!("~{anim}"),
        })
        .collect()
}

#[test]
fn a_frame_lets_go_before_it_takes_hold_and_restarts_what_it_plays() {
    let mut presence = Presence::default();
    let mut ops = Vec::new();
    presence.frame(1, &["stance_a", "stance_b"], &[], &mut ops);
    assert_eq!(names(&ops), ["+stance_a", "+stance_b"]);

    ops.clear();
    presence.frame(
        1,
        &["stance_b"],
        &[Play {
            clip: "swing",
            hold_at: None,
        }],
        &mut ops,
    );
    assert_eq!(
        names(&ops),
        ["-stance_a", "-swing", "+swing"],
        "the swing is not held"
    );

    ops.clear();
    let draw = Play {
        clip: "draw",
        hold_at: Some(1.0),
    };
    presence.frame(1, &["draw"], &[draw], &mut ops);
    assert_eq!(names(&ops), ["-stance_b", "-draw", "+draw", "@draw:1"]);

    ops.clear();
    presence.frame(1, &["draw"], &[], &mut ops);
    assert!(
        ops.is_empty(),
        "a held clip is left alone: {:?}",
        names(&ops)
    );
}

fn sword_and_shield() -> Kit {
    let melee = Melee {
        damage: 4.0,
        knockback: 0.0,
        reach: 1.5,
        windup: 6,
        cooldown: 20,
        clip: "slash".into(),
    };
    Kit {
        held: [Some("x:sword".into()), Some("x:shield".into())],
        grips: [None, None],
        walk_stances: [None, None],
        stances: [Some("stance_melee".into()), Some("stance_shield".into())],
        swing: Some(Swing {
            hand: Some(Hand::Main),
            melee,
        }),
        bow: None,
        shield: Some(Shield {
            hand: Hand::Off,
            guard: Guard {
                arc_deg: 70.0,
                raise: [1, 2],
                lower: [1, 2],
                clip: "guard".into(),
                impact_clip: "jolt".into(),
                sound: None,
            },
        }),
    }
}

#[test]
fn an_item_grip_survives_guard_recoil_and_its_hands_attack() {
    let mut kit = sword_and_shield();
    kit.grips[1] = Some("grip".into());
    kit.swing.as_mut().unwrap().hand = Some(Hand::Off);
    let mut fight = Fight::default();
    assert!(wanted(&kit, &fight, false).contains(&"grip"));
    fight.guard.raised = true;
    assert!(wanted(&kit, &fight, false).contains(&"grip"));
    fight.guard.recoil_until = Some(9);
    assert!(wanted(&kit, &fight, false).contains(&"grip"));
    fight.guard.raised = false;
    fight.act = Act::Swing {
        impact: 6,
        until: 20,
        landed: false,
    };
    let layers = wanted(&kit, &fight, false);
    assert!(layers.contains(&"grip"));
    assert!(!layers.contains(&"stance_shield"));
}

#[test]
fn an_action_or_a_raised_guard_takes_over_its_own_hand_only() {
    let kit = sword_and_shield();
    let mut fight = Fight::default();
    assert_eq!(
        wanted(&kit, &fight, false),
        ["stance_melee", "stance_shield"]
    );

    fight.act = Act::Swing {
        impact: 6,
        until: 20,
        landed: false,
    };
    assert_eq!(
        wanted(&kit, &fight, false),
        ["stance_shield"],
        "the sword arm swings"
    );

    fight.act = Act::Idle;
    fight.guard.raised = true;
    assert_eq!(wanted(&kit, &fight, false), ["stance_melee", "guard"]);
    fight.guard.recoil_until = Some(9);
    assert_eq!(
        wanted(&kit, &fight, false),
        ["stance_melee", "guard"],
        "recoil layers over the held guard without dropping the arm"
    );

    let mut bare = kit.clone();
    bare.swing.as_mut().unwrap().hand = None;
    let mut swinging = Fight::default();
    swinging.act = Act::Swing {
        impact: 6,
        until: 20,
        landed: false,
    };
    assert!(
        wanted(&bare, &swinging, false).is_empty(),
        "a bare-handed swing uses both arms"
    );
}

#[test]
fn walking_changes_only_the_stance_of_an_unused_hand() {
    let mut kit = sword_and_shield();
    kit.walk_stances = [Some("walk_main".into()), Some("walk_off".into())];
    let mut fight = Fight::default();
    assert_eq!(wanted(&kit, &fight, true), ["walk_main", "walk_off"]);
    assert_eq!(
        wanted(&kit, &fight, false),
        ["stance_melee", "stance_shield"]
    );
    fight.guard.raised = true;
    assert_eq!(wanted(&kit, &fight, true), ["walk_main", "guard"]);
    fight.act = Act::Swing {
        impact: 6,
        until: 20,
        landed: false,
    };
    assert_eq!(wanted(&kit, &fight, true), ["guard"]);
    fight.guard.raised = false;
    assert_eq!(wanted(&kit, &fight, true), ["walk_off"]);
    kit.walk_stances[1] = None;
    assert_eq!(wanted(&kit, &fight, true), ["stance_shield"]);
}

#[test]
fn bow_display_tracks_the_draw_and_restores_both_hands_afterward() {
    use crate::skeleton::combat::held_display;
    use crate::skeleton::kit::{Bow, Ranged};
    let ranged: Ranged = parse_row_data(
        r#"{
        "ammo":"x:arrow", "draw":12, "speed":20, "min_range":1,
        "max_range":20, "rest":5, "clip_draw":"draw", "clip_loose":"loose",
        "pull":["x:pull1","x:pull2","x:pull3"]
    }"#,
    )
    .unwrap();
    let mut kit = sword_and_shield();
    kit.held[1] = Some("x:bow".into());
    kit.bow = Some(Bow {
        hand: Hand::Off,
        ranged,
        flight: Default::default(),
    });
    let mut fight = Fight::default();
    fight.act = Act::Draw {
        release: 112,
        give_up: 150,
    };
    for (tick, expected) in [
        (100, "x:bow"),
        (104, "x:pull1"),
        (108, "x:pull2"),
        (111, "x:pull2"),
        (112, "x:pull3"),
        (130, "x:pull3"),
    ] {
        assert_eq!(
            held_display(&kit, &fight, tick),
            [Some("x:sword"), Some(expected)]
        );
    }
    let mut presence = Presence::default();
    assert!(presence.display(held_display(&kit, &fight, 112)).is_some());
    assert!(
        presence.display(held_display(&kit, &fight, 113)).is_none(),
        "unchanged stages do not republish"
    );
    fight.act = Act::Loose {
        until: 140,
        clip_live: true,
    };
    assert_eq!(
        held_display(&kit, &fight, 131),
        [Some("x:sword"), Some("x:bow")]
    );
    assert!(presence.display(held_display(&kit, &fight, 131)).is_some());
    fight.act = Act::Idle;
    assert_eq!(
        held_display(&kit, &fight, 132),
        [Some("x:sword"), Some("x:bow")]
    );
    kit.bow.as_mut().unwrap().ranged.pull[1].clear();
    fight.act = Act::Draw {
        release: 112,
        give_up: 150,
    };
    assert_eq!(
        held_display(&kit, &fight, 108)[1],
        Some("x:pull1"),
        "missing art holds the previous frame"
    );
}
