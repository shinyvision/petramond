use crate::skeleton::aim::Flight;
use crate::skeleton::kit::{
    admit, pick, Guard, Hand, Kit, Loadout, LoadoutSpec, Melee, Ranged, Wield,
};

fn spec(name: &str, main: Option<&str>, off: Option<&str>) -> LoadoutSpec {
    LoadoutSpec {
        name: name.into(),
        main: main.map(Into::into),
        off: off.map(Into::into),
        weight: 1,
        watch: None,
    }
}

fn melee(damage: f32, cooldown: u32, clip: &str) -> Melee {
    Melee {
        damage,
        knockback: 0.0,
        reach: 1.5,
        windup: 1,
        cooldown,
        clip: clip.into(),
    }
}

fn loadout(name: &str, weight: u32, watch: u32) -> Loadout {
    Loadout {
        name: name.into(),
        weight,
        watch,
        kit: Kit::default(),
    }
}

#[test]
fn a_loadout_naming_an_unknown_item_is_dropped_whole_and_names_stay_unique() {
    let specs = vec![
        spec("axe", Some("a:axe"), None),
        spec("bow", None, Some("b:bow")),
        spec("axe_shield", Some("a:axe"), Some("b:shield")),
        spec("axe", Some("a:axe"), None),
        spec("bare", None, None),
    ];
    let (kept, dropped) = admit(specs, |item| item.starts_with("a:"));
    let names: Vec<&str> = kept.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["axe", "bare"], "row order kept, one per name");
    assert_eq!(dropped.len(), 3);
    assert!(dropped[0].contains("b:bow"), "{dropped:?}");
    assert!(dropped[1].contains("b:shield"), "{dropped:?}");
    assert!(dropped[2].contains("twice"), "{dropped:?}");
}

#[test]
fn the_weighted_pick_lands_every_ticket_on_its_own_loadout() {
    let loadouts = [loadout("a", 2, 0), loadout("b", 0, 0), loadout("c", 3, 5)];
    let picks: Vec<Option<usize>> = (0..5).map(|roll| pick(&loadouts, false, roll)).collect();
    assert_eq!(picks, [Some(0), Some(0), Some(2), Some(2), Some(2)]);
    assert_eq!(pick(&loadouts, false, 5), Some(0), "the roll wraps");
    assert!(
        (0..50).all(|roll| pick(&loadouts, true, roll) == Some(2)),
        "a watch post weighs by the watch weights"
    );
    let unwatched = [loadout("a", 1, 0), loadout("b", 1, 0)];
    assert_eq!(
        pick(&unwatched, true, 1),
        Some(1),
        "with no watch weights at all a watch post uses the plain ones"
    );
    assert_eq!(pick(&[loadout("a", 0, 0)], false, 7), None);
    assert_eq!(pick(&[], true, 7), None);
}

#[test]
fn a_kit_takes_the_hardest_hitting_melee_and_falls_back_to_bare_hands() {
    let sword = Wield {
        stance: Some("stance_melee".into()),
        melee: Some(melee(4.0, 20, "slash")),
        ..Wield::default()
    };
    let shield = Wield {
        grip: Some("grip".into()),
        walk_stance: Some("carry".into()),
        stance: Some("stance_shield".into()),
        melee: Some(melee(2.0, 30, "bash")),
        guard: Some(Guard {
            arc_deg: 70.0,
            raise: [1, 2],
            lower: [1, 2],
            clip: "guard".into(),
            impact_clip: "jolt".into(),
            sound: None,
        }),
        ..Wield::default()
    };
    let wield = |item: &str| match item {
        "x:sword" => Some(sword.clone()),
        "x:shield" => Some(shield.clone()),
        _ => None,
    };
    let bare = melee(1.0, 20, "swipe");
    let kit = Kit::of(
        [Some("x:shield".into()), Some("x:sword".into())],
        wield,
        |_| None,
        Some(&bare),
    );
    let swing = kit.swing.expect("a melee item attacks");
    assert_eq!(kit.grips, [Some("grip".into()), None]);
    assert_eq!(kit.walk_stances, [Some("carry".into()), None]);
    assert_eq!(swing.hand, Some(Hand::Off), "the sword out-hits the bash");
    assert_eq!(swing.melee.clip, "slash");
    assert_eq!(kit.shield.map(|s| s.hand), Some(Hand::Main));
    assert_eq!(
        kit.stances,
        [Some("stance_shield".into()), Some("stance_melee".into())]
    );

    let empty = Kit::of(
        [None, Some("x:unknown".into())],
        wield,
        |_| None,
        Some(&bare),
    );
    let swing = empty.swing.expect("bare hands still fight");
    assert_eq!((swing.hand, swing.melee.clip.as_str()), (None, "swipe"));
    assert_eq!(
        empty.held[1].as_deref(),
        Some("x:unknown"),
        "still drawn in the hand"
    );
}

#[test]
fn a_bow_whose_ammo_is_unknown_cannot_shoot() {
    let bow = Wield {
        ranged: Some(Ranged {
            ammo: "x:arrow".into(),
            pull: vec![],
            draw: 20,
            speed: 28.0,
            spread: 0.0,
            min_range: 3.0,
            max_range: 24.0,
            rest: 20,
            clip_draw: "draw".into(),
            clip_loose: "loose".into(),
        }),
        ..Wield::default()
    };
    let wield = |_: &str| Some(bow.clone());
    let kit = Kit::of([None, Some("x:bow".into())], wield, |_| None, None);
    assert!(kit.bow.is_none());
    let kit = Kit::of(
        [None, Some("x:bow".into())],
        wield,
        |_| Some(Flight::default()),
        None,
    );
    assert_eq!(kit.bow.map(|b| b.hand), Some(Hand::Off));
}

#[test]
fn a_wield_row_that_cannot_work_is_refused() {
    let mut bad = melee(3.0, 10, "slash");
    bad.windup = 11;
    assert!(bad.check().is_err(), "a windup longer than the cooldown");
    assert!(melee(3.0, 10, "").check().is_err(), "no clip name");
    assert!(melee(3.0, 10, "slash").check().is_ok());
}
