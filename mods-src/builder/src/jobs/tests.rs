use super::*;
use crate::content::INFO_DATA;
use crate::host::fake::rows::{AIR, CHEST, STONE, TABLE};
use crate::project::{Hold, Note, Phase, Project};
use crate::supplies::Shortfall;
use crate::testing::{Session, ASSET, CHEST_AT, HOME, OWNER, ROW, TABLE_AT};
use crate::worker::{Step, PROJECT_TAG};

fn row_site() -> (Session, ProjectId) {
    Session::row()
}

fn brief(session: &mut Session, id: ProjectId) -> Brief {
    session.builder.projects.get(id).expect("a project").brief()
}

fn stock(session: &Session, item: &str, count: u8) {
    session
        .world
        .give(ContainerAddress::Block(CHEST_AT), item, count);
}

fn short(count: u32, name: &str) -> Refusal {
    Refusal::Short(Shortfall {
        count,
        name: name.into(),
        more: false,
    })
}

fn at_work(session: &mut Session, id: ProjectId) {
    session.builder.projects.update(id, |p| {
        p.summon(TABLE_AT);
        p.emerged();
    });
}

#[test]
fn a_design_compiles_within_the_ticks_sections_and_names_the_project() {
    let (mut session, id) = row_site();
    let project = brief(&mut session, id);
    session.builder.jobs.attend(&project, 1);
    let mut sections = 2;
    session.builder.compile(id, &mut sections);
    assert_eq!(sections, 0, "two stored sections spent");
    assert!(session.builder.jobs.map[&id].survey.is_none());
    session.builder.compile(id, &mut sections);
    assert_eq!(session.builder.jobs.map[&id].design.compiled(), 2, "none left to spend");

    let mut sections = 8;
    session.builder.compile(id, &mut sections);
    assert_eq!(sections, 7);
    let job = &session.builder.jobs.map[&id];
    assert!(job.design.complete && job.survey.is_some());
    assert!(job.summary().is_none(), "surveyed from the next step on");
    assert_eq!(session.builder.projects.get(id).unwrap().title, "Row");
    let blueprint = session.world.container(ContainerAddress::Block(TABLE_AT))[0]
        .clone()
        .expect("the blueprint stays in the slot");
    assert!(blueprint
        .data
        .contains(&(INFO_DATA.to_owned(), b"Row".to_vec())));
    assert_eq!(session.builder.projects.bound(&blueprint), Some(id));
}

#[test]
fn a_design_that_failed_is_not_compiled_again() {
    let (mut session, id) = row_site();
    session.world.schematic_lookup(
        ASSET,
        SchematicLookup::Failed {
            reason: "Corrupt".into(),
        },
    );
    let project = brief(&mut session, id);
    session.builder.jobs.attend(&project, 1);
    let mut sections = 8;
    session.builder.compile(id, &mut sections);
    assert_eq!(session.builder.jobs.map[&id].failed.as_deref(), Some("Corrupt"));
    assert_eq!(
        session.builder.admission(&project, 1),
        Err(Refusal::Design("Corrupt".into()))
    );
}

#[test]
fn a_repositioned_draft_starts_its_design_afresh() {
    let (mut session, id) = row_site();
    session.job(id);
    session
        .builder
        .projects
        .update(id, |p| p.origin = Some([4, 0, 4]));
    let project = brief(&mut session, id);
    let job = session.builder.jobs.attend(&project, 2).expect("still anchored");
    assert_eq!(job.design.origin, [4, 0, 4]);
    assert!(!job.design.complete && job.survey.is_none());
    session.builder.jobs.map.get_mut(&id).unwrap().crew.last_mob = Some(55);
    assert_eq!(session.builder.jobs.by_mob(55).map(|j| j.id), Some(id));
    assert!(session.builder.jobs.by_mob(56).is_none());
}

#[test]
fn start_is_refused_until_the_chests_cover_the_bill() {
    let (mut session, id) = row_site();
    let project = brief(&mut session, id);
    assert_eq!(
        session.builder.admission(&project, 1),
        Err(Refusal::CheckingSite)
    );
    session.job(id);
    assert_eq!(session.builder.admission(&project, 2), Err(short(3, "Stone")));
    assert_eq!(project_done(&session, id), 0.0);
    stock(&session, "petramond:stone", 3);
    assert_eq!(
        session.builder.admission(&project, 2),
        Err(short(3, "Stone")),
        "answered once a tick"
    );
    assert_eq!(session.builder.admission(&project, 3), Ok(()));

    session
        .world
        .put(ContainerAddress::Block(TABLE_AT), 0, None);
    assert_eq!(
        session.builder.admission(&project, 4),
        Err(Refusal::BlueprintNotTabled)
    );
    let loose = session.builder.projects.create(OWNER.into(), TABLE_AT);
    let loose = brief(&mut session, loose);
    assert_eq!(
        session.builder.admission(&loose, 4),
        Err(Refusal::NotPositioned)
    );
    assert_eq!(
        Refusal::NotLoaded(4).to_string(),
        "4 blocks are not loaded yet"
    );
}

fn project_done(session: &Session, id: ProjectId) -> f32 {
    session.builder.jobs.map[&id].done()
}

#[test]
fn start_waits_for_the_whole_site_to_load() {
    let (mut session, id) = row_site();
    session.world.unload([2, 0, 0], [2, 0, 0]);
    session.job(id);
    let project = brief(&mut session, id);
    assert_eq!(
        session.builder.admission(&project, 2),
        Err(Refusal::NotLoaded(1))
    );
}

#[test]
fn start_never_breaks_a_chest_holding_items() {
    let (mut session, id) = row_site();
    session.world.chest([0, 0, 0], 1);
    session
        .world
        .give(ContainerAddress::Block([0, 0, 0]), "petramond:dirt", 1);
    session.job(id);
    let project = brief(&mut session, id);
    assert_eq!(session.builder.admission(&project, 2), Err(Refusal::Guarded));
}

#[test]
fn a_chest_nobody_can_read_is_no_shortfall() {
    let (mut session, id) = row_site();
    session.world.set([-1, 0, -3], CHEST);
    session.job(id);
    let project = brief(&mut session, id);
    assert_eq!(
        session.builder.admission(&project, 2),
        Err(Refusal::CheckingSupplies)
    );
}

#[test]
fn a_finished_job_with_nothing_left_is_not_started_again() {
    let (mut session, id) = row_site();
    session.world.fill([0, 0, 0], [2, 0, 0], STONE);
    session.job(id);
    assert_eq!(project_done(&session, id), 1.0);
    session.builder.projects.update(id, |p| {
        p.summon(TABLE_AT);
        p.gone_home();
    });
    let project = brief(&mut session, id);
    assert!(project.resumable());
    assert_eq!(
        session.builder.admission(&project, 2),
        Err(Refusal::NothingLeft)
    );
}

/// A second table on the other side of the same chest, its job under way.
#[test]
fn what_another_working_job_sharing_the_chests_still_needs_is_not_offered() {
    let (mut session, a) = row_site();
    let other_table = [2, 0, -3];
    session.world.set(other_table, TABLE);
    let b = session
        .builder
        .projects
        .create(OWNER.into(), other_table);
    session.builder.projects.update(b, |p| {
        p.asset = Some(ASSET);
        p.origin = Some([0, 0, 5]);
        p.summon(other_table);
        p.emerged();
    });
    session.job(a);
    session.job(b);
    stock(&session, "petramond:stone", 4);
    let project = brief(&mut session, a);
    let offered = session.builder.available(&project, 2);
    assert_eq!(offered.get(&("petramond:stone".into(), Vec::new())), Some(&1));
    assert_eq!(session.builder.admission(&project, 2), Err(short(2, "Stone")));
}

#[test]
fn start_summons_a_golem_beside_the_table_once() {
    let (mut session, id) = row_site();
    assert_eq!(session.builder.start(99), Err(Refusal::NoProject));
    session.job(id);
    stock(&session, "petramond:stone", 3);
    assert_eq!(session.builder.start(id), Ok(()));
    let project = session.builder.projects.get(id).unwrap().clone();
    assert_eq!((project.phase(), project.home), (Phase::Emerging, HOME));
    let golem = session.builder.jobs.map[&id].crew.mob.expect("a golem");
    assert_eq!(
        session.world.tag(golem, PROJECT_TAG),
        Some(MobTagValue::I64(id as i64))
    );
    assert_eq!(
        session.world.tag(golem, crate::worker::FULL_HEALTH_TAG),
        Some(MobTagValue::F64(60.0))
    );
    assert_eq!(session.builder.jobs.map[&id].crew.step, Step::Emerge { t: 0 });
    let feet = session.world.mob_pos(golem).unwrap();
    assert!(feet[1] < 0.0, "it starts under the ground");
    assert_eq!(session.builder.start(id), Err(Refusal::AlreadyStarted));
}

#[test]
fn a_table_with_no_ground_around_it_has_nowhere_to_summon_from() {
    let mut session = Session::flat(10);
    session.world.schematic(ASSET, "Row", [3, 1, 1], &ROW, 1);
    let table = [0, 20, -3];
    let id = session.draft(table, [0, 0, 0]);
    let blueprint = session.blueprint(id);
    session
        .world
        .put(ContainerAddress::Block(table), 0, Some(blueprint));
    session.world.chest([1, 20, -3], 4);
    session
        .world
        .give(ContainerAddress::Block([1, 20, -3]), "petramond:stone", 3);
    session.job(id);
    assert_eq!(
        session.builder.start(id),
        Err(Refusal::Summon(
            "There is no open ground beside the table for the golem".into()
        ))
    );
    assert_eq!(
        session.builder.projects.get(id).map(Project::phase),
        Some(Phase::Draft)
    );
}

#[test]
fn pause_and_resume_hold_and_release_a_working_job() {
    let (mut session, id) = row_site();
    session.builder.pause(id);
    assert_eq!(brief(&mut session, id).hold, None, "a draft is not paused");
    session.job(id);
    at_work(&mut session, id);
    session.builder.pause(id);
    let project = session.builder.projects.get(id).unwrap();
    assert_eq!((project.hold(), Note::read(&project.note)), (Some(Hold::Player), Note::Paused));
    assert_eq!(session.builder.resume(id, [3, 0, -3]), Ok(()));
    let project = session.builder.projects.get(id).unwrap();
    assert_eq!((project.hold(), project.note.as_str(), project.table), (None, "", [3, 0, -3]));
    assert_eq!(session.builder.resume(id, TABLE_AT), Ok(()), "nothing held");

    session
        .builder
        .projects
        .update(id, |p| p.hold_for(Hold::Blueprint, "Missing blueprint"));
    assert_eq!(
        session.builder.resume(id, TABLE_AT),
        Err(Refusal::GolemLostBlueprint)
    );
    assert_eq!(session.builder.resume(99, TABLE_AT), Err(Refusal::NoProject));
}

#[test]
fn a_supplies_hold_lifts_once_the_chests_cover_the_rest() {
    let (mut session, id) = row_site();
    session.job(id);
    at_work(&mut session, id);
    session
        .builder
        .projects
        .update(id, |p| p.hold_for(Hold::Supplies, "Missing 3x Stone"));
    assert_eq!(session.builder.resume(id, TABLE_AT), Err(short(3, "Stone")));
    stock(&session, "petramond:stone", 3);
    session.world.set_now(2);
    assert_eq!(session.builder.resume(id, TABLE_AT), Ok(()));
    assert_eq!(session.builder.projects.get(id).unwrap().hold(), None);
}

#[test]
fn a_working_job_keeps_its_missing_note_current() {
    let (mut session, id) = row_site();
    session.job(id);
    at_work(&mut session, id);
    let project = brief(&mut session, id);
    session.builder.check_supplies(&project, 2);
    assert_eq!(session.builder.projects.get(id).unwrap().note, "Missing 3x Stone");

    session
        .builder
        .projects
        .update(id, |p| p.hold_for(Hold::Supplies, "Missing 3x Stone"));
    stock(&session, "petramond:stone", 3);
    let project = brief(&mut session, id);
    session.builder.check_supplies(&project, 3);
    let project = session.builder.projects.get(id).unwrap();
    assert_eq!((project.hold(), project.note.as_str()), (None, ""));
}

#[test]
fn a_storage_hold_lifts_once_a_chest_has_room() {
    let (mut session, id) = row_site();
    session.job(id);
    at_work(&mut session, id);
    for _ in 0..4 {
        stock(&session, "petramond:dirt", 64);
    }
    session
        .builder
        .projects
        .update(id, |p| p.hold_for(Hold::Storage, Note::ChestsFull));
    let project = brief(&mut session, id);
    session.builder.check_supplies(&project, 2);
    assert_eq!(session.builder.projects.get(id).unwrap().hold(), Some(Hold::Storage));
    session
        .world
        .put(ContainerAddress::Block(CHEST_AT), 3, None);
    session.builder.check_supplies(&project, 3);
    let project = session.builder.projects.get(id).unwrap();
    assert_eq!((project.hold(), project.note.as_str()), (None, ""));
}

#[test]
fn a_dead_golem_holds_its_job() {
    let (mut session, id) = row_site();
    session.job(id);
    let golem = session.golem(id, HOME, HOME);
    session.builder.died(golem + 1);
    assert_eq!(session.builder.jobs.map[&id].crew.mob, Some(golem), "another mob");
    session.builder.died(golem);
    assert_eq!(session.builder.jobs.map[&id].crew.mob, None);
    let project = brief(&mut session, id);
    assert!(project.lost_worker());
}

#[test]
fn a_job_whose_table_is_broken_is_called_off() {
    let (mut session, id) = row_site();
    session.job(id);
    at_work(&mut session, id);
    // Due on the tick its id falls on, of every forty.
    session.world.unload(TABLE_AT, TABLE_AT);
    session.builder.cancel_tableless(&[id], 41);
    assert!(!brief(&mut session, id).cancelling, "unloaded is not gone");
    session.world.state_mut().unloaded.clear();
    session.world.set(TABLE_AT, AIR);
    session.builder.cancel_tableless(&[id], 40);
    assert!(!brief(&mut session, id).cancelling, "not its tick");
    session.builder.cancel_tableless(&[id], 41);
    let project = session.builder.projects.get(id).unwrap();
    assert_eq!((project.phase(), project.cancelling()), (Phase::Returning, true));
    assert_eq!(Note::read(&project.note), Note::TableGone);
}

#[test]
fn a_watched_draft_is_compiled_surveyed_and_ghosted_in_one_tick() {
    let (mut session, id) = row_site();
    session.builder.tick();
    assert!(session.builder.jobs.map.is_empty(), "nobody looks at it");
    session.world.state_mut().viewers.push(GuiViewerData {
        player_id: PlayerId(1),
        kind: crate::keys::table::KIND.into(),
        anchor: Some(ContainerAddress::Block(TABLE_AT)),
    });
    session.world.set_now(2);
    session.builder.tick();
    let job = &session.builder.jobs.map[&id];
    let summary = job.summary().expect("compiled and surveyed");
    assert_eq!(summary.bill.values().sum::<u32>(), 3);
    assert_eq!(session.builder.projects.get(id).unwrap().title, "Row");
    assert!(session
        .world
        .deeds()
        .contains(&crate::host::fake::Deed::Ghost(crate::project::tag_of(id), true)));
}
