use super::*;
use crate::mob::brain::AiMob;
use crate::mob::Mob;
use petramond_math::world_pos::WorldPos;
use petramond_world::block::Block;
use petramond_world::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};

/// A grass floor at `y = 63` over one chunk, so footholds sit at `y = 64`.
fn flat_world() -> ServerWorld {
    let mut world = ServerWorld::new(0, 1);
    let mut chunk = Chunk::new(0, 0);
    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            chunk.set_block(x, 63, z, Block::Grass);
        }
    }
    world.insert_chunk_for_test(ChunkPos::new(0, 0), chunk);
    world
}

fn with_budget(budget: &PathBudget) -> NavInputs<'_> {
    NavInputs {
        budget: Some(budget),
        ..NavInputs::none()
    }
}

#[test]
fn a_one_cell_goal_drift_keeps_the_route_until_the_hold_expires() {
    let world = flat_world();
    let start = IVec3::new(1, 64, 1);
    let goal = IVec3::new(10, 64, 1);
    let mut nav = Navigator::new(1, 0.25, 0.9);
    nav.update_goal_when_supported(Some(goal), start, &world, true, &NavInputs::none());
    assert_eq!(nav.recomputes(), 1);

    // The target stepped one cell sideways: the live route is kept.
    let drifted = IVec3::new(10, 64, 2);
    for _ in 0..GOAL_DRIFT_REPATH_TICKS {
        nav.update_goal_when_supported(Some(drifted), start, &world, true, &NavInputs::none());
    }
    assert_eq!(nav.recomputes(), 1, "no search inside the drift hold");
    assert_eq!(nav.path().last(), Some(&goal));

    nav.update_goal_when_supported(Some(drifted), start, &world, true, &NavInputs::none());
    assert_eq!(
        nav.recomputes(),
        2,
        "the hold expires and the drift is re-searched"
    );
    assert_eq!(nav.path().last(), Some(&drifted));
}

#[test]
fn a_bigger_goal_move_is_searched_at_once() {
    let world = flat_world();
    let start = IVec3::new(1, 64, 1);
    let mut nav = Navigator::new(1, 0.25, 0.9);
    nav.update_goal_when_supported(
        Some(IVec3::new(10, 64, 1)),
        start,
        &world,
        true,
        &NavInputs::none(),
    );
    let moved = IVec3::new(10, 64, 5);
    nav.update_goal_when_supported(Some(moved), start, &world, true, &NavInputs::none());
    assert_eq!(nav.recomputes(), 2);
    assert_eq!(nav.path().last(), Some(&moved));
}

#[test]
fn a_goal_on_the_route_ahead_shortens_it_without_a_search() {
    let world = flat_world();
    let start = IVec3::new(1, 64, 1);
    let goal = IVec3::new(12, 64, 1);
    let mut nav = Navigator::new(1, 0.25, 0.9);
    nav.update_goal_when_supported(Some(goal), start, &world, true, &NavInputs::none());
    let route = nav.path().to_vec();
    let ahead = route[route.len() / 2];

    nav.update_goal_when_supported(Some(ahead), start, &world, true, &NavInputs::none());
    assert_eq!(
        nav.recomputes(),
        1,
        "no search for a goal already on the route"
    );
    assert_eq!(nav.path(), &route[..=route.len() / 2]);
    assert!(!nav.is_idle());
}

#[test]
fn an_exhausted_budget_suspends_the_search_and_the_mob_waits_for_it() {
    let world = flat_world();
    let start = IVec3::new(1, 64, 1);
    let goal = IVec3::new(14, 64, 14);
    let mut reference = Navigator::new(1, 0.25, 0.9);
    reference.update_goal_when_supported(Some(goal), start, &world, true, &NavInputs::none());

    // Room for only a few expansions per tick.
    let budget = PathBudget::with_capacity(5);
    let mut nav = Navigator::new(1, 0.25, 0.9);
    budget.refill(0);
    nav.update_goal_when_supported(Some(goal), start, &world, true, &with_budget(&budget));
    assert!(
        nav.search_waiting(),
        "the search did not fit and was parked"
    );
    assert_eq!(nav.recomputes(), 0);
    assert!(!nav.is_idle(), "a mob waiting on its route is not idle");

    let mut ticks = 1;
    while nav.search_waiting() {
        budget.refill(1);
        nav.update_goal_when_supported(Some(goal), start, &world, true, &with_budget(&budget));
        ticks += 1;
        assert!(ticks < 500, "a waiting search always progresses");
    }
    assert!(ticks > 2, "the search really was spread over several ticks");
    assert_eq!(nav.recomputes(), 1);
    assert_eq!(
        nav.path(),
        reference.path(),
        "slicing never changes the route"
    );
}

#[test]
fn a_search_resumed_after_the_mob_moved_starts_the_route_where_it_stands() {
    let world = flat_world();
    let start = IVec3::new(1, 64, 1);
    let goal = IVec3::new(12, 64, 1);
    let budget = PathBudget::with_capacity(5);
    let mut nav = Navigator::new(1, 0.25, 0.9);
    budget.refill(0);
    nav.update_goal_when_supported(Some(goal), start, &world, true, &with_budget(&budget));
    assert!(nav.search_waiting());
    let moved = IVec3::new(2, 64, 1);
    while nav.search_waiting() {
        budget.refill(1);
        nav.update_goal_when_supported(Some(goal), moved, &world, true, &with_budget(&budget));
    }
    assert_eq!(nav.path().first(), Some(&moved));
    assert_eq!(nav.path().last(), Some(&goal));
}

#[test]
fn waiting_searches_get_a_reserved_share_fresh_requests_cannot_spend() {
    let budget = PathBudget::with_capacity(1000);
    budget.refill(0);
    assert_eq!(
        budget.grant(false),
        1000,
        "nobody waiting: the whole budget is open"
    );

    budget.refill(3);
    assert_eq!(
        budget.grant(false),
        500,
        "fresh requests see only the open half"
    );
    assert_eq!(budget.grant(true), 1000, "a continuation sees both halves");

    // Fresh traffic drains the open half; the reserve survives it.
    budget.spend(600, false);
    assert_eq!(budget.grant(false), 0);
    assert_eq!(budget.grant(true), 500);

    // A continuation drains the reserve first.
    budget.refill(1);
    budget.spend(100, true);
    assert_eq!(budget.grant(false), 500);
    assert!(budget.grant(true) < 1000);
}

#[test]
fn species_tuning_sets_the_refresh_cadence_and_node_cap() {
    let world = flat_world();
    let start = IVec3::new(1, 64, 1);
    let goal = IVec3::new(10, 64, 1);
    let tuning = NavTuning {
        max_nodes: 500,
        repath_ticks: 5,
    };
    tuning.validate().expect("valid tuning");
    let mut nav = Navigator::new(1, 0.25, 0.9).with_tuning(tuning);
    assert_eq!(nav.params.max_nodes, 500);
    nav.update_goal_when_supported(Some(goal), start, &world, true, &NavInputs::none());
    for _ in 0..4 {
        nav.update_goal_when_supported(Some(goal), start, &world, true, &NavInputs::none());
    }
    assert_eq!(nav.recomputes(), 1);
    nav.update_goal_when_supported(Some(goal), start, &world, true, &NavInputs::none());
    assert_eq!(nav.recomputes(), 2, "refreshed on the species cadence");
}

#[test]
fn nav_tuning_rows_parse_with_defaults_and_reject_nonsense() {
    let parsed: NavTuning = serde_json::from_str(r#"{"repath_ticks": 40}"#).expect("parses");
    assert_eq!(parsed.repath_ticks, 40);
    assert_eq!(parsed.max_nodes, NavTuning::default().max_nodes);
    assert!(parsed.validate().is_ok());
    assert!(serde_json::from_str::<NavTuning>(r#"{"max_node": 1}"#).is_err());
    for bad in [
        NavTuning {
            max_nodes: 10,
            ..NavTuning::default()
        },
        NavTuning {
            repath_ticks: 0,
            ..NavTuning::default()
        },
    ] {
        assert!(bad.validate().is_err(), "{bad:?}");
    }
}

#[test]
fn entity_costs_price_only_nearby_bodies_through_the_spatial_index() {
    let near = AiMob {
        id: 2,
        kind: Mob::Sheep,
        pos: WorldPos::new(4.5, 64.0, 1.5),
        active: true,
        tags: Default::default(),
    };
    let far = AiMob {
        id: 3,
        pos: WorldPos::new(80.5, 64.0, 1.5),
        ..near.clone()
    };
    let mobs = MobSnapshot::from_mobs([near, far]);
    let inputs = NavInputs {
        self_id: 1,
        mobs: &mobs,
        ..NavInputs::none()
    };
    let mut costs = FxHashMap::default();
    costs.insert(IVec3::new(99, 99, 99), 5);
    entity_cell_costs(&inputs, IVec3::new(1, 64, 1), &mut costs);
    assert!(
        costs.contains_key(&IVec3::new(4, 64, 1)),
        "the near sheep is priced"
    );
    assert!(
        !costs.keys().any(|c| c.x > 60),
        "the far one is out of range"
    );
    assert!(
        !costs.contains_key(&IVec3::new(99, 99, 99)),
        "the map is reset"
    );
}
