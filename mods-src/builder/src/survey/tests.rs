use super::*;
use crate::design::Progress;
use crate::host::fake::rows::{DIRT, STONE};
use crate::host::fake::stack;
use crate::testing::{Session, ASSET};

const STONE_ROW: [([i32; 3], &str); 3] = [
    ([0, 0, 0], "petramond:stone"),
    ([1, 0, 0], "petramond:stone"),
    ([2, 0, 0], "petramond:stone"),
];

fn row(session: &Session, extra: Option<&str>) -> Design {
    let mut cells = STONE_ROW.to_vec();
    cells.extend(extra.map(|name| ([3, 0, 0], name)));
    session
        .world
        .schematic(ASSET, "Row", [cells.len() as i32, 1, 1], &cells, 64);
    let mut design = Design::new(ASSET, [0, 0, 0], 0);
    assert!(matches!(design.compile(8), Progress::Ready));
    design
}

fn stone() -> ItemKey {
    ("petramond:stone".into(), Vec::new())
}

fn bill(survey: &Survey) -> Vec<(ItemKey, u32)> {
    let summary = survey.summary().expect("every unit measured");
    summary.bill.iter().map(|(k, n)| (k.clone(), *n)).collect()
}

#[test]
fn nothing_is_summed_until_every_unit_was_measured() {
    let session = Session::flat(6);
    let design = row(&session, None);
    let mut survey = Survey::new(&design);
    assert!(survey.summary().is_none());
    assert_eq!(survey.open(), 3, "unchecked work is open work");
    survey.step(&design, 2, 1, 0, &[], true);
    assert!(survey.summary().is_none(), "one unit is still unmeasured");
    survey.step(&design, 2, 2, 0, &[], false);
    let summary = survey.summary().expect("the first pass is over");
    assert_eq!((summary.open, summary.unchecked), (3, 0));
    assert_eq!(bill(&survey), vec![(stone(), 3)]);
    assert!(survey
        .known
        .iter()
        .all(|k| *k == Known::Place(vec![stack("petramond:stone", 1)])));
}

#[test]
fn a_changed_cell_is_measured_again_and_the_totals_follow() {
    let session = Session::flat(6);
    let design = row(&session, None);
    let mut survey = Survey::new(&design);
    survey.step(&design, 16, 1, 0, &[], true);

    session.world.set([1, 0, 0], STONE);
    survey.step(&design, 16, 2, 0, &[[1, 0, 0]], false);
    assert_eq!(bill(&survey), vec![(stone(), 2)]);
    assert_eq!(survey.open(), 2);

    session.world.set([2, 0, 0], DIRT);
    survey.step(&design, 16, 3, 0, &[[2, 0, 0]], false);
    let summary = survey.summary().unwrap();
    assert_eq!((summary.clear, summary.guarded, summary.open), (1, 0, 2));
    assert_eq!(summary.clear_blocks.get(&DIRT), Some(&1));
    assert_eq!(bill(&survey), vec![(stone(), 2)]);

    session.world.chest([0, 0, 0], 4);
    session
        .world
        .give(ContainerAddress::Block([0, 0, 0]), "petramond:dirt", 3);
    survey.step(&design, 16, 4, 0, &[[0, 0, 0]], false);
    let summary = survey.summary().unwrap();
    assert_eq!((summary.clear, summary.guarded), (2, 1));
    assert!(matches!(
        survey.known[design.unit_at([0, 0, 0]).unwrap()],
        Known::Clear {
            at: [0, 0, 0],
            holds_items: true,
            ..
        }
    ));

    session.world.set([0, 0, 0], STONE);
    session.world.set([2, 0, 0], STONE);
    survey.step(&design, 16, 5, 0, &[[0, 0, 0], [2, 0, 0]], false);
    let summary = survey.summary().unwrap();
    assert!(summary.bill.is_empty(), "a paid-off entry leaves the bill");
    assert!(summary.clear_blocks.is_empty());
    assert_eq!((summary.open, summary.clear, summary.guarded), (0, 0, 0));
}

#[test]
fn a_tick_the_survey_sat_out_measures_everything_again() {
    let session = Session::flat(6);
    let design = row(&session, None);
    let mut survey = Survey::new(&design);
    survey.step(&design, 16, 1, 0, &[], true);
    session.world.set([0, 0, 0], STONE);
    survey.step(&design, 16, 2, 0, &[], false);
    assert_eq!(bill(&survey), vec![(stone(), 3)]);
    survey.step(&design, 16, 4, 0, &[], false);
    assert_eq!(bill(&survey), vec![(stone(), 2)]);
}

#[test]
fn a_unit_on_unloaded_ground_is_asked_again_on_the_retry_round() {
    let session = Session::flat(6);
    let design = row(&session, None);
    session.world.unload([2, -1, 0], [2, 0, 0]);
    let mut survey = Survey::new(&design);
    survey.step(&design, 16, 1, 12, &[], true);
    let far = design.unit_at([2, 0, 0]).unwrap();
    assert_eq!(survey.known[far], Known::Unloaded);
    assert_eq!(survey.summary().unwrap().unchecked, 1);
    session.world.state_mut().unloaded.clear();
    survey.step(&design, 16, 2, 12, &[], false);
    assert!(matches!(survey.known[far], Known::Place(_)));
    assert_eq!(survey.summary().unwrap().unchecked, 0);
    assert_eq!(bill(&survey), vec![(stone(), 3)]);
}

#[test]
fn unsupported_work_is_counted_once_and_never_asked_about() {
    let session = Session::flat(6);
    let design = row(&session, Some("petramond:water"));
    let mut survey = Survey::new(&design);
    survey.step(&design, 16, 1, 0, &[], true);
    let summary = survey.summary().expect("the supported units were measured");
    assert_eq!(summary.unsupported, 1);
    assert_eq!(summary.first_unsupported, "Fluids cannot be built");
    assert_eq!(summary.open, 4);
    let water = design.unit_at([3, 0, 0]).unwrap();
    assert_eq!(
        survey.known[water],
        Known::Unsupported("Fluids cannot be built".into())
    );
}

#[test]
fn a_bill_counts_items_by_their_exact_data() {
    let plain = stack("petramond:stone", 2);
    let marked = ItemStackData {
        data: vec![("builder:project".into(), vec![1])],
        ..stack("petramond:stone", 5)
    };
    let mut summary = Summary::default();
    summary.count(
        &Known::Place(vec![plain.clone(), marked.clone()]),
        &[],
        true,
    );
    summary.count(&Known::Place(vec![plain.clone()]), &[], true);
    assert_eq!(summary.bill.get(&key_of(&plain)), Some(&4));
    assert_eq!(summary.bill.get(&key_of(&marked)), Some(&5));
    summary.count(&Known::Place(vec![plain.clone(), marked]), &[], false);
    assert_eq!(summary.bill.len(), 1, "the marked stack is paid off");
    assert_eq!(summary.open, 1);
    summary.count(&Known::Place(vec![stack("petramond:stone", 9)]), &[], false);
    summary.count(&Known::Place(Vec::new()), &[], false);
    assert!(summary.bill.is_empty());
    assert_eq!(summary.open, 0);
}
