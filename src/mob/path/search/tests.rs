use super::*;
use crate::mob::path::find_path_nav;

fn solid(c: IVec3) -> bool {
    c.y < 1 || (c.x == 5 && c.z != 9 && c.y < 3) || (c.x == 12 && c.z == 3 && c.y < 4)
}

fn no_fluid(_: IVec3) -> bool {
    false
}

fn any_step(_: IVec3, _: IVec3) -> bool {
    true
}

fn cost(c: IVec3) -> u32 {
    if c.x == 2 && c.z == 2 {
        200
    } else {
        0
    }
}

type FnProbes = SearchProbes<
    'static,
    fn(IVec3) -> bool,
    fn(IVec3) -> bool,
    fn(IVec3) -> bool,
    fn(IVec3, IVec3) -> bool,
    fn(IVec3) -> u32,
>;

fn probes() -> FnProbes {
    const SOLID: fn(IVec3) -> bool = solid;
    const FLUID: fn(IVec3) -> bool = no_fluid;
    const STEP: fn(IVec3, IVec3) -> bool = any_step;
    const COST: fn(IVec3) -> u32 = cost;
    SearchProbes {
        solid: &SOLID,
        support: &SOLID,
        fluid: &FLUID,
        step_allowed: &STEP,
        cell_cost: &COST,
    }
}

fn sliced(
    start: IVec3,
    goal: IVec3,
    params: PathParams,
    grant: usize,
) -> (Vec<IVec3>, usize, usize) {
    let mut search = NavSearch::default();
    search.begin(start, goal);
    let (mut slices, mut total) = (0, 0);
    loop {
        slices += 1;
        let (poll, spent) = search.run(params, &probes(), grant);
        assert!(spent <= grant, "a slice never overspends its grant");
        total += spent;
        if let SearchPoll::Done(route) = poll {
            return (route, slices, total);
        }
    }
}

#[test]
fn a_sliced_search_finds_exactly_the_one_shot_route() {
    let params = PathParams::default();
    let start = IVec3::new(0, 1, 0);
    for goal in [
        IVec3::new(10, 1, 0),
        IVec3::new(9, 1, 14),
        IVec3::new(12, 4, 3),
    ] {
        let one_shot = find_path_nav(
            start, goal, params, &solid, &solid, no_fluid, any_step, cost,
        );
        for grant in [1, 7, 64, usize::MAX] {
            let (route, slices, _) = sliced(start, goal, params, grant);
            assert_eq!(route, one_shot, "goal {goal:?} in slices of {grant}");
            if grant == 1 {
                assert!(slices > 1, "a one-expansion grant must suspend");
            }
        }
    }
}

#[test]
fn the_node_cap_spans_every_slice() {
    let params = PathParams {
        max_nodes: 50,
        ..PathParams::default()
    };
    let (_, _, total) = sliced(IVec3::new(0, 1, 0), IVec3::new(12, 4, 3), params, 3);
    assert_eq!(total, 50);
}

#[test]
fn a_reused_search_forgets_its_previous_run() {
    let params = PathParams::default();
    let mut search = NavSearch::default();
    let (first, _) = search.solve(IVec3::new(0, 1, 0), IVec3::new(9, 1, 14), params, &probes());
    let (second, _) = search.solve(IVec3::new(3, 1, 3), IVec3::new(1, 1, 1), params, &probes());
    let fresh = find_path_nav(
        IVec3::new(3, 1, 3),
        IVec3::new(1, 1, 1),
        params,
        &solid,
        &solid,
        no_fluid,
        any_step,
        cost,
    );
    assert_eq!(first.last(), Some(&IVec3::new(9, 1, 14)));
    assert_eq!(second, fresh);
}
