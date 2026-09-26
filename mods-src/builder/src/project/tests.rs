use super::*;
use crate::content::PROJECT_DATA;

#[test]
fn a_project_record_round_trips() {
    let mut project = Project::new(7, "ada".into(), [1, -2, 3]);
    project.asset = Some([9; 32]);
    project.title = "Townhouse".into();
    project.origin = Some([-40, 64, 12]);
    project.turns = 3;
    project.summon([1, -1, 3]);
    project.emerged();
    project.cancel();
    project.hold_for(Hold::Storage, Note::ChestsFull);
    project.show_ghost = false;
    project.scaffolds = vec![[1, 2, 3], [4, 5, 6]];
    assert_eq!(
        (project.phase(), project.cancelling(), project.worker()),
        (Phase::Returning, true, true)
    );
    assert_eq!(Project::decode(&project.encode()), Some(project));
}

/// Worlds hold project records written as the flat field list: they must
/// still read, and read as the same job.
#[test]
fn a_record_written_flat_reads_as_the_same_job() {
    let flat = Record {
        id: 7,
        owner: "ada".into(),
        table: [1, -2, 3],
        asset: Some([9; 32]),
        title: "Townhouse".into(),
        origin: Some([-40, 64, 12]),
        turns: 3,
        phase: Phase::Working,
        hold: Some(Hold::Worker),
        cancelling: false,
        home: [1, -1, 3],
        worker: false,
        scaffolds: vec![[1, 2, 3]],
        note: Note::GolemDied.into(),
        started: true,
        show_ghost: true,
    };
    let bytes = mod_sdk::encode(&flat).unwrap();
    let project = Project::decode(&bytes).expect("a flat record reads");
    assert!(project.brief().lost_worker());
    assert_eq!(project.encode(), bytes, "and is written back byte for byte");
}

#[test]
fn a_note_reads_back_as_what_was_said() {
    for note in [
        Note::None,
        Note::Paused,
        Note::GolemDied,
        Note::TableGone,
        Note::ChestsFull,
        Note::Missing("Missing 3x Stone and more".into()),
        Note::Text("The golem's hands are full".into()),
    ] {
        assert_eq!(Note::read(&String::from(note.clone())), note);
    }
    // The golem's own "Missing blueprint" is no shortfall of supplies.
    assert!(!Note::read("Missing blueprint").is_shortfall());
}

fn draft() -> Project {
    Project::new(1, "ada".into(), [0, 0, 0])
}

/// What the state machine answers, in one tuple.
fn reads(p: &Project) -> (Phase, Option<Hold>, bool, bool, bool) {
    (p.phase(), p.hold(), p.cancelling(), p.worker(), p.started())
}

#[test]
fn a_job_runs_from_draft_to_complete() {
    let mut p = draft();
    assert_eq!(reads(&p), (Phase::Draft, None, false, false, false));
    assert!(p.brief().open_to_change());
    p.hold_for(Hold::Player, Note::Paused);
    assert_eq!((p.hold(), p.note.as_str()), (None, ""), "a draft is never held");

    p.note = "stale".into();
    p.summon([1, 0, 0]);
    assert_eq!(reads(&p), (Phase::Emerging, None, false, true, true));
    assert_eq!((p.home, p.note.as_str()), ([1, 0, 0], ""));
    assert!(!p.brief().open_to_change());
    p.emerged();
    assert_eq!(p.phase(), Phase::Working);
    p.hold_for(Hold::Supplies, "Missing 3x Stone");
    assert_eq!(p.hold(), Some(Hold::Supplies));
    assert!(Note::read(&p.note).is_shortfall());
    p.release();
    assert_eq!(p.hold(), None);
    p.wind_down("1 block was lost after it was placed".into());
    assert_eq!(p.phase(), Phase::Returning);
    p.burrow();
    assert_eq!(p.phase(), Phase::Burrowing);
    p.gone_home();
    assert_eq!(reads(&p), (Phase::Complete, None, false, false, true));
    assert_eq!(p.note, "1 block was lost after it was placed");
    let brief = p.brief();
    assert!(brief.resumable() && !brief.lost_worker() && !brief.open_to_change());
}

#[test]
fn a_job_called_off_winds_down_as_far_as_it_got() {
    let mut p = draft();
    p.cancel();
    assert_eq!(reads(&p), (Phase::Cancelled, None, false, false, false));
    assert!(!p.brief().resumable(), "never started, nothing to resume");

    let mut p = draft();
    p.summon([1, 0, 0]);
    p.emerged();
    p.hold_for(Hold::Player, Note::Paused);
    p.cancel();
    assert_eq!(reads(&p), (Phase::Returning, None, true, true, true));
    p.burrow();
    p.gone_home();
    assert_eq!(reads(&p), (Phase::Cancelled, None, false, false, true));
    assert!(p.resumable());

    // Called off while it rises: it comes up, then goes straight home.
    let mut p = draft();
    p.summon([1, 0, 0]);
    p.cancel();
    assert_eq!(reads(&p), (Phase::Emerging, None, true, true, true));
    p.emerged();
    assert_eq!(p.phase(), Phase::Returning);

    let mut p = draft();
    p.summon([1, 0, 0]);
    p.burrow();
    p.cancel();
    assert_eq!((p.phase(), p.cancelling()), (Phase::Burrowing, true));
}

#[test]
fn a_dead_golem_holds_the_job_for_the_next() {
    let mut p = draft();
    p.golem_died();
    assert_eq!(reads(&p), (Phase::Draft, None, false, false, false));

    for stage in [Project::emerged as fn(&mut Project), Project::burrow, |_: &mut Project| {}] {
        let mut p = draft();
        p.summon([1, 0, 0]);
        stage(&mut p);
        p.golem_died();
        assert_eq!(
            reads(&p),
            (Phase::Working, Some(Hold::Worker), false, false, true),
            "rising, working or sinking, it is work again"
        );
        assert_eq!(Note::read(&p.note), Note::GolemDied);
        assert!(p.brief().lost_worker());
    }

    // A new golem takes the job up where the last left it.
    let mut p = draft();
    p.summon([1, 0, 0]);
    p.emerged();
    p.golem_died();
    p.summon([2, 0, 0]);
    assert_eq!(reads(&p), (Phase::Emerging, None, false, true, true));
    assert_eq!(p.note, "");

    // With no golem out, calling it off simply ends it.
    let mut p = draft();
    p.summon([1, 0, 0]);
    p.golem_died();
    p.cancel();
    assert_eq!(reads(&p), (Phase::Cancelled, None, false, false, true));
}

#[test]
fn a_tag_names_its_project() {
    assert_eq!(tag_of(12), "builder:project/12");
    assert_eq!(id_of_tag(&tag_of(12)), Some(12));
    assert_eq!(id_of_tag("builder:project/twelve"), None);
    assert_eq!(id_of_tag("other:12"), None);
    assert_eq!(draft().brief().at([4, 5, 6]).table, [4, 5, 6]);
    let mut anchored = draft();
    assert_eq!(anchored.anchored(), None);
    anchored.asset = Some([3; 32]);
    anchored.origin = Some([1, 2, 3]);
    anchored.turns = 2;
    assert_eq!(anchored.anchored(), Some(([3; 32], [1, 2, 3], 2)));
}

/// Finish project `id` the ordinary way.
fn finish(projects: &mut Projects, id: ProjectId) {
    projects.update(id, |p| {
        p.summon(p.table);
        p.emerged();
        p.wind_down(String::new());
        p.burrow();
        p.gone_home();
    });
}

#[test]
fn projects_are_numbered_and_listed_while_live_across_a_reload() {
    let _session = crate::testing::Session::flat(1);
    let mut projects = Projects::load();
    let a = projects.create("ada".into(), [0, 0, 0]);
    let b = projects.create("bo".into(), [5, 0, 0]);
    assert_eq!((a, b), (1, 2));
    assert_eq!(projects.live().collect::<Vec<_>>(), vec![1, 2]);
    projects.update(a, Project::cancel);
    assert_eq!(projects.live().collect::<Vec<_>>(), vec![2]);

    let mut again = Projects::load();
    assert_eq!(again.live().collect::<Vec<_>>(), vec![2]);
    assert_eq!(again.get(a).map(Project::phase), Some(Phase::Cancelled));
    assert_eq!(again.get(b).map(|p| p.owner.clone()), Some("bo".into()));
    assert_eq!(again.create("cy".into(), [0, 0, 0]), 3);
    assert_eq!(again.binding(b), projects.binding(b), "the world keeps its nonce");
}

#[test]
fn a_listed_project_whose_record_is_gone_is_no_project() {
    let session = crate::testing::Session::flat(1);
    let mut projects = Projects::load();
    let id = projects.create("ada".into(), [0, 0, 0]);
    session.world.state_mut().kv.remove(&tag_of(id));
    let mut again = Projects::load();
    assert_eq!(again.live().collect::<Vec<_>>(), vec![id]);
    assert!(again.get(id).is_none());
    assert_eq!(again.live().count(), 0);
    assert_eq!(Projects::load().live().count(), 0, "and the index says so");
}

#[test]
fn a_blueprint_is_bound_to_its_world_and_its_project() {
    let _session = crate::testing::Session::flat(1);
    let mut projects = Projects::load();
    let id = projects.create("ada".into(), [0, 0, 0]);
    let bound = |data: Vec<(String, Vec<u8>)>| {
        projects.bound(&ItemStackData {
            item: crate::keys::BLUEPRINT.into(),
            count: 1,
            data,
        })
    };
    let binding = projects.binding(id);
    assert_eq!(bound(vec![(PROJECT_DATA.into(), binding.clone())]), Some(id));
    let mut elsewhere = binding.clone();
    elsewhere[0] ^= 1;
    assert_eq!(bound(vec![(PROJECT_DATA.into(), elsewhere)]), None, "another world's");
    assert_eq!(bound(vec![(PROJECT_DATA.into(), binding[..8].to_vec())]), None);
    assert_eq!(bound(Vec::new()), None, "a blank blueprint");
}

#[test]
fn the_newest_finished_project_stays_as_its_tables_report() {
    let _session = crate::testing::Session::flat(1);
    let mut projects = Projects::load();
    let table = [0, 0, 0];
    let ids: Vec<ProjectId> = (0..3)
        .map(|_| projects.create("ada".into(), table))
        .collect();
    finish(&mut projects, ids[0]);
    finish(&mut projects, ids[1]);
    assert_eq!(projects.report_at(table), Some(ids[1]), "drafts are no report");
    assert_eq!(projects.active_at(table), None);
    assert_eq!(projects.report_at([9, 9, 9]), None);
    projects.update(ids[2], |p| p.summon(table));
    assert_eq!(projects.active_at(table), Some(ids[2]));
    assert_eq!(projects.report_at(table), Some(ids[1]), "the report stands");

    projects.sweep();
    assert!(ids.iter().all(|id| projects.peek(*id).is_some()), "all asked about");
    projects.sweep();
    assert!(projects.peek(ids[0]).is_none());
    assert!(projects.peek(ids[1]).is_some() && projects.peek(ids[2]).is_some());
    assert_eq!(
        projects.get(ids[0]).map(Project::phase),
        Some(Phase::Complete),
        "read again when asked"
    );
}

#[test]
fn a_tables_report_is_found_after_a_reload() {
    let _session = crate::testing::Session::flat(1);
    let mut projects = Projects::load();
    let table = [3, 0, -2];
    let older = projects.create("ada".into(), table);
    let newer = projects.create("ada".into(), table);
    finish(&mut projects, newer);
    finish(&mut projects, older);
    let reloaded = Projects::load();
    assert!(reloaded.peek(newer).is_none(), "nothing is read yet");
    assert_eq!(reloaded.report_at(table), Some(newer), "the newest, not the last");
    assert_eq!(reloaded.report_at([0, 0, 0]), None);
}
