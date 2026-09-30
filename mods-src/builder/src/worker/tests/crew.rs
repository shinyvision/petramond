use crate::host::prelude::*;
use crate::testing::Session;
use crate::worker::crew::{Aloft, Deferrals, Faces, Stamped};
use crate::worker::tuning::every::SEARCH_EVERY;
use crate::worker::tuning::waits::FACELESS_SPACING;
use crate::worker::waiting::{Probe, Waiting};
use crate::worker::{Crew, Pillar, Step, Task, GOAL_TAG, HOLD_TAG, PROJECT_TAG};

#[test]
fn work_set_aside_waits_its_time() {
    let mut deferrals = Deferrals::default();
    let task = Task::Unit(3);
    deferrals.defer(task, 50);
    assert!(deferrals.deferred(task, 49));
    assert!(!deferrals.deferred(task, 50));
    assert!(!deferrals.deferred(Task::Unit(4), 0));

    deferrals.strike(task, [1, 0, 1]);
    assert!(deferrals.blind(task, [1, 0, 1]));
    assert!(!deferrals.blind(task, [1, 0, 2]));
    assert!(!deferrals.blind(Task::Unit(4), [1, 0, 1]));
}

#[test]
fn a_search_that_found_nothing_stands_until_something_moves() {
    let mut deferrals = Deferrals::default();
    let task = Task::Scaffold([0, 3, 0]);
    deferrals.found_nowhere(task, [0, 0, 0], 100, true);
    assert_eq!(deferrals.still_nowhere(task, [0, 0, 0], 99), Some(true));
    assert_eq!(
        deferrals.still_nowhere(task, [1, 0, 0], 99),
        None,
        "stood elsewhere"
    );
    assert_eq!(
        deferrals.still_nowhere(task, [0, 0, 0], 100),
        None,
        "forgiven"
    );
    assert_eq!(deferrals.still_nowhere(Task::Unit(0), [0, 0, 0], 99), None);
    deferrals.site_changed();
    assert_eq!(
        deferrals.still_nowhere(task, [0, 0, 0], 99),
        None,
        "the site moved"
    );
}

#[test]
fn a_faceless_unit_is_propped_after_spaced_refusals() {
    let mut faces = Faces::default();
    assert!(!faces.faceless_try(1, 10));
    assert!(
        !faces.faceless_try(1, 20),
        "asks close together are one try"
    );
    assert!(!faces.faceless_try(1, 10 + FACELESS_SPACING));
    assert!(faces.faceless_try(1, 10 + 2 * FACELESS_SPACING));
    assert!(!faces.faceless_try(2, 10), "counted per unit");

    assert!(!faces.waits_on_the_build(1));
    faces.floating.insert(1);
    faces.hangs.insert(4);
    faces.unheld.insert(5);
    assert!([1, 4, 5].iter().all(|u| faces.waits_on_the_build(*u)));

    faces.propped.insert(1, vec![[0, 0, 0], [0, 1, 0]]);
    assert_eq!(faces.unprop(1), Some(vec![[0, 0, 0], [0, 1, 0]]));
    assert!(!faces.floating.contains(&1));
    assert_eq!(
        faces.faceless.count_spaced(1, 0, FACELESS_SPACING),
        1,
        "tries start over"
    );
    assert_eq!(faces.unprop(1), None);
}

#[test]
fn getting_off_a_pillar_forgets_as_much_as_the_way_off_calls_for() {
    let pillar = Pillar {
        column: [3, 4],
        base: 0,
        top: 5,
        exit: [2, 0, 4],
        onward: None,
    };
    let up = || Aloft {
        perch: Some(pillar),
        climbed_for: Some((Task::Unit(1), 2)),
        descending: Some(3),
        ..Aloft::default()
    };
    let mut aloft = up();
    aloft.dismount_to_raise();
    assert_eq!(
        (aloft.perch, aloft.climbed_for, aloft.descending),
        (None, Some((Task::Unit(1), 2)), Some(3))
    );
    let mut aloft = up();
    aloft.dismount_down();
    assert_eq!(
        (aloft.perch, aloft.climbed_for, aloft.descending),
        (None, Some((Task::Unit(1), 2)), None)
    );
    let mut aloft = up();
    aloft.dismount_lost();
    assert_eq!((aloft.perch, aloft.climbed_for), (None, None));

    assert_eq!(pillar.top_cell(), [3, 5, 4]);
    assert_eq!(pillar.foot_cell(), [3, 0, 4]);
    assert!(pillar.holds([3, 4, 4]) && !pillar.holds([3, 5, 4]) && !pillar.holds([3, 2, 5]));
    assert!(pillar.on_column([3, 40, 4]));
}

#[test]
fn a_reading_goes_stale_after_its_period() {
    let never = Stamped::<u32>::default();
    assert!(never.stale(0, 1000), "never read");
    let read = Stamped { at: 100, value: 7 };
    assert!(!read.stale(139, 40));
    assert!(read.stale(140, 40));
}

#[test]
fn a_wait_is_told_in_the_owners_words_only_when_worth_telling() {
    assert_eq!(Waiting::Resupply.told(), Some("Waiting for materials"));
    assert_eq!(
        Waiting::GroundLoading.told(),
        Some("Waiting for the ground to load")
    );
    assert_eq!(Waiting::Probe(Probe::Walk).told(), None);
    assert_eq!(
        Waiting::Probe(Probe::Walk).pondering(),
        Some("Working out a way to the next block")
    );
    assert_eq!(Waiting::Nothing.pondering(), None);
    assert_eq!(Waiting::Nothing.label(), "");
    assert_eq!(Waiting::Probe(Probe::Pillar).label(), "pillar search busy");
}

#[test]
fn a_crew_finds_its_golem_by_its_tag_and_clears_what_it_wore() {
    let session = Session::flat(4);
    let golem = session.world.golem_at([0, 0, 0]);
    {
        let mut state = session.world.state_mut();
        let tags = &mut state.mobs.get_mut(&golem).unwrap().tags;
        tags.insert(PROJECT_TAG.into(), MobTagValue::I64(9));
        tags.insert(GOAL_TAG.into(), [1, 2, 3].to_tag());
        tags.insert(HOLD_TAG.into(), MobTagValue::Bool(true));
    }
    let mut crew = Crew {
        step: Step::Descend { since: 1 },
        ..Crew::default()
    };
    assert_eq!(crew.find(5, 8), None, "another project's golem");
    assert_eq!(crew.find(6, 9), None, "looked a tick ago");
    assert_eq!(crew.find(5 + SEARCH_EVERY, 9), Some(golem));
    assert_eq!((crew.mob, crew.last_mob), (Some(golem), Some(golem)));
    assert_eq!(crew.step, Step::Plan);
    assert_eq!(crew.pace.progress_at, 5 + SEARCH_EVERY);
    assert_eq!(session.world.tag(golem, GOAL_TAG), None);
    assert_eq!(session.world.tag(golem, HOLD_TAG), None);
    assert!(session.world.tag(golem, PROJECT_TAG).is_some());

    session.world.state_mut().mobs.remove(&golem);
    let now = 5 + SEARCH_EVERY;
    assert_eq!(crew.find(now + 1, 9), None);
    assert_eq!((crew.mob, crew.last_mob), (None, Some(golem)));
    let next = session.world.golem_at([1, 0, 0]);
    session
        .world
        .state_mut()
        .mobs
        .get_mut(&next)
        .unwrap()
        .tags
        .insert(PROJECT_TAG.into(), MobTagValue::I64(9));
    assert_eq!(crew.find(now + 2, 9), None);
    assert_eq!(crew.find(now + SEARCH_EVERY, 9), Some(next));
}
