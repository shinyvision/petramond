use super::{at_work, working};
use crate::project::{Hold, Note, Project};
use crate::testing::{short_of_stone, HOME};
use crate::worker::trouble::{self, Trouble};
use crate::worker::tuning::patience::{STUCK_AFTER, THINK_AFTER};
use crate::worker::waiting::Waiting;
use crate::worker::{Crew, Step};

fn project() -> Project {
    let (mut session, id, _) = working(HOME);
    session.builder.projects.get(id).unwrap().clone()
}

#[test]
fn a_golem_thinks_a_while_before_it_counts_as_stuck() {
    let working = project();
    let crew = Crew::default();
    assert_eq!(trouble::of(&working, &crew, THINK_AFTER), None);
    assert_eq!(trouble::of(&working, &crew, THINK_AFTER + 1), Some(Trouble::Thinking));
    assert_eq!(trouble::of(&working, &crew, STUCK_AFTER + 1), Some(Trouble::Stuck));

    let getting_out = Crew {
        step: Step::Hop {
            to: [0, 0, 0],
            since: 0,
        },
        ..Crew::default()
    };
    assert_eq!(trouble::of(&working, &getting_out, 1), Some(Trouble::Thinking));

    let mut asked = Crew::default();
    asked.presence.asked_about = Some(None);
    assert_eq!(
        trouble::of(&working, &asked, THINK_AFTER + 1),
        None,
        "what it said when asked stands"
    );
}

#[test]
fn only_a_hold_the_player_can_lift_is_trouble() {
    let mut paused = project();
    paused.hold_for(Hold::Player, Note::Paused);
    assert_eq!(trouble::of(&paused, &Crew::default(), STUCK_AFTER + 1), None);
    let mut short = project();
    short.hold_for(Hold::Supplies, short_of_stone(3));
    assert_eq!(trouble::of(&short, &Crew::default(), 1), Some(Trouble::Stuck));
    let mut rising = project();
    rising.summon(HOME);
    assert_eq!(trouble::of(&rising, &Crew::default(), STUCK_AFTER + 1), None);
}

#[test]
fn the_reason_is_whatever_was_last_said_or_what_it_is_doing() {
    let working = project();
    let crew = Crew::default();
    assert_eq!(
        trouble::reason(&working, &crew, Some(Trouble::Stuck)),
        "The golem has found nothing it can do"
    );
    assert_eq!(
        trouble::reason(&working, &crew, Some(Trouble::Thinking)),
        "Working out what to do next"
    );
    assert_eq!(trouble::reason(&working, &crew, None), "Building");

    let mut waiting = Crew::default();
    waiting.why = Waiting::Resupply;
    assert_eq!(
        trouble::reason(&working, &waiting, Some(Trouble::Stuck)),
        "Waiting for materials"
    );
    let mut noted = working.clone();
    noted.note = short_of_stone(3);
    assert_eq!(
        trouble::reason(&noted, &waiting, Some(Trouble::Stuck)),
        "Missing 3x Stone",
        "the table's note first"
    );

    let mut held = working.clone();
    held.hold_for(Hold::Worker, Note::None);
    assert_eq!(trouble::reason(&held, &crew, None), "Paused");
    held.hold_for(Hold::Player, Note::Paused);
    assert_eq!(trouble::reason(&held, &waiting, None), "Paused");

    let mut home = working.clone();
    home.wind_down(Note::None);
    assert_eq!(trouble::reason(&home, &crew, None), "Returning materials");
    home.burrow();
    assert_eq!(trouble::reason(&home, &crew, None), "Burrowing home");
    let mut rising = working;
    rising.summon(HOME);
    assert_eq!(trouble::reason(&rising, &crew, None), "Digging itself out");
}

#[test]
fn the_mark_over_its_head_follows_its_trouble() {
    let (mut session, id, golem) = working(HOME);
    let body = session.body(golem);
    at_work(&mut session, id, |ctx, _, job| {
        trouble::show(ctx, &mut job.crew, &body, Some(Trouble::Thinking));
        assert!(job.crew.presence.mark.is_some());
    });
    assert_eq!(session.world.state().mobs[&golem].drawn, 1);
    at_work(&mut session, id, |ctx, _, job| {
        trouble::show(ctx, &mut job.crew, &body, None);
        assert!(job.crew.presence.mark.is_none());
    });
    assert_eq!(session.world.state().mobs[&golem].drawn, 0);
}
