use super::*;

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
