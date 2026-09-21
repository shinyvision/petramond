use super::*;
use mod_api::Route;
use petramond_math::facing::Facing;
use petramond_math::world_pos::WorldPos;
use petramond_world::block::Block;

#[test]
fn idle_until_given_a_goal() {
    let nav = Navigator::new(1, 0.25, 0.9);
    assert!(nav.is_idle());
}

#[test]
fn arriving_consumes_waypoints_then_goes_idle() {
    let mut nav = Navigator::new(1, 0.25, 0.9);
    // Hand-build a 2-step path so we don't need a World: start (0,1,0) -> (1,1,0).
    nav.path = vec![IVec3::new(0, 1, 0), IVec3::new(1, 1, 0)];
    nav.index = 1;
    nav.goal = Some(IVec3::new(1, 1, 0));
    nav.path_reaches_goal = true;
    // Standing on the waypoint: it's consumed and the nav goes idle.
    let on_wp = WorldPos::new(1.5, 1.0, 0.5);
    let (wish, jump) = nav.follow(on_wp, true);
    assert_eq!(wish, Vec3::ZERO);
    assert!(!jump);
    assert!(nav.is_idle(), "arrived -> idle");
}

#[test]
fn steers_toward_a_distant_waypoint() {
    let mut nav = Navigator::new(1, 0.25, 0.9);
    nav.path = vec![IVec3::new(0, 1, 0), IVec3::new(5, 1, 0)];
    nav.index = 1;
    nav.goal = Some(IVec3::new(5, 1, 0));
    let (wish, jump) = nav.follow(WorldPos::new(0.5, 1.0, 0.5), true);
    assert!(wish.x > 0.9, "heads +X toward the waypoint: {wish:?}");
    assert!(!jump, "flat move needs no jump");
}

#[test]
fn jumps_when_close_to_a_step_up() {
    let mut nav = Navigator::new(1, 0.22, 0.9);
    // Waypoint one block up and just ahead.
    nav.path = vec![IVec3::new(0, 1, 0), IVec3::new(1, 2, 0)];
    nav.index = 1;
    nav.goal = Some(IVec3::new(1, 2, 0));
    let (_wish, jump) = nav.follow(WorldPos::new(0.7, 1.0, 0.5), true);
    assert!(jump, "should jump for a nearby one-block step up");
    // But not while airborne.
    let (_w2, jump_air) = nav.follow(WorldPos::new(0.7, 1.0, 0.5), false);
    assert!(!jump_air, "no jump while off the ground");
}

/// The hop-off-a-ledge rubber-band regression: a ballistic arc carries
/// the body past waypoints — including between steered ticks — and the
/// cursor must advance past every one of them (monotonic projection
/// consumption, [`Navigator::advance_cursor`]) no matter how far beyond
/// a centre the flight lands; steering back to a place the body has
/// already been past must be impossible.
#[test]
fn a_ballistic_pass_advances_the_cursor_and_never_turns_the_mob_back() {
    let mut nav = Navigator::new(1, 0.35, 0.9);
    nav.path = vec![
        IVec3::new(0, 1, 0),
        IVec3::new(1, 1, 0),
        IVec3::new(2, 1, 0),
        IVec3::new(3, 1, 0),
    ];
    nav.index = 1;
    nav.goal = Some(IVec3::new(3, 1, 0));
    nav.path_reaches_goal = true;

    let (wish, _) = nav.follow(WorldPos::new(0.6, 1.0, 0.5), true);
    assert!(wish.x > 0.9, "heads toward the waypoint: {wish:?}");
    // The arc carries the body past TWO waypoint centres, unsteered,
    // landing far outside any arrive window.
    for x in [0.9, 1.4, 2.0, 2.6] {
        nav.advance_cursor(WorldPos::new(x, 1.3, 0.5));
    }
    // Steering resumes well past both — the wish must aim FORWARD.
    let (wish, _) = nav.follow(WorldPos::new(2.7, 1.0, 0.5), true);
    assert!(
        wish.x > 0.9,
        "passed waypoints were consumed mid-flight; no turning back: {wish:?}"
    );

    // The same guarantee on a STEERED overshoot (the hushjaw backtrack
    // shape): a fast walker that steps far past a waypoint centre in one
    // tick keeps going forward, never orbits back to it.
    let mut nav = Navigator::new(1, 0.45, 0.9);
    nav.path = vec![
        IVec3::new(0, 1, 0),
        IVec3::new(1, 1, 0),
        IVec3::new(2, 1, 0),
    ];
    nav.index = 1;
    nav.goal = Some(IVec3::new(2, 1, 0));
    nav.path_reaches_goal = true;
    let (wish, _) = nav.follow(WorldPos::new(1.2, 1.0, 0.5), true);
    assert!(wish.x > 0.9, "closing in: {wish:?}");
    let (wish, _) = nav.follow(WorldPos::new(1.9, 1.0, 0.5), true);
    assert!(
        wish.x > 0.9,
        "a big step past the centre keeps aiming forward: {wish:?}"
    );
}

/// Moving a lot is not going somewhere: a ballistic gait ping-ponging in
/// a pocket its stride cannot resolve keeps raw displacement high every
/// tick, so only GOAL-progress liveness can abandon the route (the
/// 2026-08-17 rabbit cliff-edge trap). An honest approach never trips it.
#[test]
fn a_route_with_movement_but_no_goal_progress_is_abandoned() {
    let mut nav = Navigator::new(1, 0.35, 0.9);
    nav.path = vec![IVec3::new(0, 1, 0), IVec3::new(8, 1, 0)];
    nav.index = 1;
    nav.goal = Some(IVec3::new(8, 1, 0));
    nav.path_reaches_goal = true;
    // Ping-pong across a whole block each call — far beyond the raw
    // displacement epsilon — while never getting nearer the goal.
    for call in 0..2 * GOAL_STALL_CALLS {
        if nav.is_idle() {
            break;
        }
        let x = if call % 2 == 0 { 0.5 } else { 1.5 };
        nav.follow(WorldPos::new(x, 1.0, 0.5), true);
    }
    assert!(
        nav.is_idle(),
        "a movement-rich, progress-free route must be abandoned"
    );

    // An honest (even slow) approach keeps the route alive well past the
    // stall budget: the best-achieved goal distance keeps improving.
    let mut nav = Navigator::new(1, 0.35, 0.9);
    nav.path = vec![IVec3::new(0, 1, 0), IVec3::new(200, 1, 0)];
    nav.index = 1;
    nav.goal = Some(IVec3::new(200, 1, 0));
    nav.path_reaches_goal = true;
    for call in 0..3 * GOAL_STALL_CALLS {
        let x = 0.5 + call as f32 * 0.05;
        nav.follow(WorldPos::new(f64::from(x), 1.0, 0.5), true);
    }
    assert!(
        !nav.is_idle(),
        "honest closing progress never reads as a stall"
    );
}

#[test]
fn vertical_bobbing_does_not_count_as_navigation_progress() {
    let mut nav = Navigator::new(1, 0.25, 0.9);
    nav.path = vec![IVec3::new(0, 1, 0), IVec3::new(5, 1, 0)];
    nav.index = 1;
    nav.goal = Some(IVec3::new(5, 1, 0));
    nav.last_pos = petramond_math::world_pos::WorldPos::new(0.5, 1.0, 0.5);

    for tick in 0..STUCK_TICKS {
        let y = 1.0 + if tick % 2 == 0 { 0.2 } else { -0.2 };
        let (wish, _jump) = nav.follow(WorldPos::new(0.5, y, 0.5), false);
        if tick + 1 < STUCK_TICKS {
            assert!(wish.x > 0.9, "still trying to move horizontally");
            assert!(!nav.is_idle(), "not abandoned before the stuck limit");
        }
    }

    assert!(
        nav.is_idle(),
        "bobbing in place should abandon the bad route so wander can choose again"
    );
}

#[test]
fn jump_trigger_accounts_for_body_width() {
    let mut nav = Navigator::new(1, 0.45, 0.9);
    // Waypoint one block up in the adjacent cell. A wide mob standing in the lower
    // cell is already close to the ledge with its front edge even though its centre
    // is still a full block from the target centre.
    nav.path = vec![IVec3::new(0, 1, 0), IVec3::new(1, 2, 0)];
    nav.index = 1;
    nav.goal = Some(IVec3::new(1, 2, 0));
    let (_wish, jump) = nav.follow(WorldPos::new(0.5, 1.0, 0.5), true);
    assert!(
        jump,
        "wider bodies jump before colliding with the step face"
    );
}

#[test]
fn wide_mob_does_not_turn_before_clearing_a_corner() {
    let mut nav = Navigator::new(1, 0.45, 0.9);
    // The route turns north at (1,1,0). A sheep-width body at x=1.25 would still
    // clip a block in the inner corner if it started the turn, so it must keep
    // steering east until much closer to the waypoint centre.
    nav.path = vec![
        IVec3::new(0, 1, 0),
        IVec3::new(1, 1, 0),
        IVec3::new(1, 1, 1),
    ];
    nav.index = 1;
    nav.goal = Some(IVec3::new(1, 1, 1));
    let (wish, jump) = nav.follow(WorldPos::new(1.25, 1.0, 0.5), true);
    assert!(
        wish.x > 0.9 && wish.z.abs() < 0.1,
        "wide mob should keep clearing the corner before turning: {wish:?}"
    );
    assert!(!jump);
}

/// A single chunk with a solid grass floor at `y = 63`, so footholds sit at
/// `y = 64` across it — enough terrain for `find_path` to route over.
fn flat_world() -> World {
    use petramond_world::chunk::{Chunk, ChunkPos};
    let mut world = World::new(0, 2);
    world.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
    for x in 0..12 {
        for z in 0..4 {
            world.set_block_world(x, 63, z, Block::Grass);
        }
    }
    world
}

fn pillar_world() -> (World, IVec3, IVec3) {
    let mut world = flat_world();
    let start = IVec3::new(1, 64, 1);
    let goal = IVec3::new(8, 66, 1);
    world.set_block_world(goal.x, 64, goal.z, Block::Stone);
    world.set_block_world(goal.x, 65, goal.z, Block::Stone);
    (world, start, goal)
}

fn world_with_door_in_wall(open: bool) -> (World, IVec3, IVec3, IVec3) {
    let mut world = flat_world();
    let door = IVec3::new(4, 64, 1);
    for x in 0..12 {
        if x == door.x {
            continue;
        }
        world.set_block_world(x, 64, door.z, Block::Stone);
        world.set_block_world(x, 65, door.z, Block::Stone);
    }
    assert!(world.place_door(door, Block::OakDoor, Facing::South));
    if open {
        assert_eq!(world.toggle_door(door), Some(door));
    }
    (world, IVec3::new(4, 64, 0), IVec3::new(4, 64, 2), door)
}

#[test]
fn closed_door_blocks_the_crossing_edge() {
    let (world, start, goal, door) = world_with_door_in_wall(false);
    let mut nav = Navigator::new(1, 0.25, 0.9);

    nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());

    assert_ne!(
        nav.path().last(),
        Some(&goal),
        "closed door must not route through the wall opening: {:?}",
        nav.path()
    );
    assert!(
        nav.path().last().is_some_and(|p| p.z <= door.z),
        "partial route should stop on the near side or in the door cell: {:?}",
        nav.path()
    );
}

#[test]
fn open_door_allows_the_cleared_crossing_edge() {
    let (world, start, goal, door) = world_with_door_in_wall(true);
    let mut nav = Navigator::new(1, 0.25, 0.9);

    nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());

    assert_eq!(nav.path().last(), Some(&goal), "open door is routeable");
    assert!(
        nav.path().contains(&door),
        "the route should pass through the door cell: {:?}",
        nav.path()
    );
}

#[test]
fn open_door_still_blocks_the_swung_edge() {
    let mut world = flat_world();
    let door = IVec3::new(4, 64, 1);
    assert!(world.place_door(door, Block::OakDoor, Facing::South));
    assert_eq!(world.toggle_door(door), Some(door));
    let start = IVec3::new(3, 64, 1);
    let goal = IVec3::new(5, 64, 1);
    let mut nav = Navigator::new(1, 0.25, 0.9);

    nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());

    assert_eq!(nav.path().last(), Some(&goal), "a detour remains possible");
    assert!(
        !nav.path()
            .windows(2)
            .any(|w| w[0] == start && w[1] == door),
        "the open door's swung slab sits on the west edge, so the route must not enter straight from the west: {:?}",
        nav.path()
    );
}

#[test]
fn a_ladder_panel_blocks_only_the_edge_it_physically_blocks() {
    // The regression this rework exists for: a ladder's block row has NO
    // collision (its 1/16 panel resolves per-facing at the world level), so
    // cell navigation used to read its cell as fully open and walk mobs
    // straight into the panel forever. The edge gate sweeps the real body
    // AABB: crossing the panel's face is refused, while the open 15/16 of
    // the same cell stays routable.
    let mut world = flat_world();
    let ladder = IVec3::new(4, 64, 1);
    for x in 0..12 {
        if x == ladder.x {
            continue;
        }
        world.set_block_world(x, 64, ladder.z, Block::Stone);
        world.set_block_world(x, 65, ladder.z, Block::Stone);
    }
    // `Block::Ladder` faces north: its panel hugs the z = 2 face of its cell.
    world.set_block_world(ladder.x, ladder.y, ladder.z, Block::Ladder);
    let start = IVec3::new(4, 64, 0);
    let mut nav = Navigator::new(1, 0.25, 0.9);

    // Entering the ladder cell from the open side is fine — the cell is NOT
    // a blanket wall.
    nav.update_goal_when_supported(Some(ladder), start, &world, true, &NavObstacles::none());
    assert_eq!(
        nav.path().last(),
        Some(&ladder),
        "the open 15/16 of a ladder cell stays routable: {:?}",
        nav.path()
    );

    // Crossing the panel's face is refused: the far side is unreachable and
    // the best-effort route never steps through the panel.
    let mut nav = Navigator::new(1, 0.25, 0.9);
    let goal = IVec3::new(4, 64, 2);
    nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
    assert_ne!(
        nav.path().last(),
        Some(&goal),
        "the mob must not plan through the ladder panel: {:?}",
        nav.path()
    );
    assert!(
        nav.path().iter().all(|c| c.z <= ladder.z),
        "the best-effort route stops on the near side of the panel: {:?}",
        nav.path()
    );
}

#[test]
fn a_touching_body_ahead_veers_the_wish_to_a_side() {
    let pos = WorldPos::new(0.5, 64.0, 0.5);
    let wish = Vec3::new(1.0, 0.0, 0.0);
    let blocking = [AiMob {
        id: 2,
        kind: crate::mob::Mob::Sheep,
        pos: WorldPos::new(1.4, 64.0, 0.5),
        active: true,
        tags: Default::default(),
    }];
    let contacts = [EntityRef::Mob(2)];

    // Dead ahead: the wish veers sideways (id-picked side), keeping forward speed.
    let out = Unstick::default().steer(wish, pos, 1, 0.45, &contacts, None, &blocking, &[]);
    assert!(out.z.abs() > 0.5, "veers sideways around the body: {out:?}");
    assert!(out.x > 0.0, "keeps forward progress: {out:?}");

    // No contact (and no running commitment): the wish passes through untouched.
    assert_eq!(
        Unstick::default().steer(wish, pos, 1, 0.45, &[], None, &blocking, &[]),
        wish
    );

    // The brain's target is never dodged.
    assert_eq!(
        Unstick::default().steer(
            wish,
            pos,
            1,
            0.45,
            &contacts,
            Some(EntityRef::Mob(2)),
            &blocking,
            &[]
        ),
        wish
    );

    // A body BEHIND the travel direction is not ours to dodge.
    let behind = [AiMob {
        id: 2,
        kind: crate::mob::Mob::Sheep,
        pos: WorldPos::new(-0.4, 64.0, 0.5),
        active: true,
        tags: Default::default(),
    }];
    assert_eq!(
        Unstick::default().steer(wish, pos, 1, 0.45, &contacts, None, &behind, &[]),
        wish
    );
}

#[test]
fn the_veer_commits_to_its_side_instead_of_flip_flopping() {
    // Contacts are recorded from the PREVIOUS tick's overlap, so a veer
    // that works erases its own trigger next tick. The latch must keep
    // rounding the peer on the SAME side while the commitment runs down —
    // the stateless version snapped straight, re-pressed, and wagged the
    // wish (and the body facing) every other tick.
    let pos = WorldPos::new(0.5, 64.0, 0.5);
    let wish = Vec3::new(1.0, 0.0, 0.0);
    let blocking = [AiMob {
        id: 2,
        kind: crate::mob::Mob::Sheep,
        pos: WorldPos::new(1.4, 64.0, 0.5),
        active: true,
        tags: Default::default(),
    }];
    let contacts = [EntityRef::Mob(2)];

    let mut latch = Unstick::default();
    let veered = latch.steer(wish, pos, 1, 0.45, &contacts, None, &blocking, &[]);
    assert!(veered.z.abs() > 0.5, "the contact tick veers: {veered:?}");

    // Contact gone: the commitment keeps the SAME veer, no snap-back.
    for _ in 0..UNSTICK_HOLD_TICKS {
        assert_eq!(
            latch.steer(wish, pos, 1, 0.45, &[], None, &blocking, &[]),
            veered,
            "the committed side holds through contact flicker"
        );
    }
    // Commitment exhausted: the wish runs straight again.
    assert_eq!(
        latch.steer(wish, pos, 1, 0.45, &[], None, &blocking, &[]),
        wish,
        "the veer expires once the contact has stayed gone"
    );

    // A body slightly to the OTHER side of the new travel line must not
    // flip the committed side while the commitment is live.
    let mut latch = Unstick::default();
    let first = latch.steer(wish, pos, 1, 0.45, &contacts, None, &blocking, &[]);
    let other_side = [AiMob {
        pos: WorldPos::new(1.3, 64.0, f64::from(0.4 - first.z.signum() * 0.2)),
        ..blocking[0].clone()
    }];
    let second = latch.steer(wish, pos, 1, 0.45, &contacts, None, &other_side, &[]);
    assert_eq!(
        first.z.signum(),
        second.z.signum(),
        "a live commitment pins the veer side: {first:?} vs {second:?}"
    );
}

#[test]
fn a_one_high_block_wall_is_routable_but_a_one_high_fence_wall_is_not() {
    // Ordinary one-block steps stay jumpable; a fence of the same height
    // must never route, or no fenced pen would hold (the by-design pen
    // rule in `nav_solid_fn`/`nav_support_fn`).
    let start = IVec3::new(4, 64, 0);
    let goal = IVec3::new(4, 64, 2);

    let mut stone_world = flat_world();
    for x in 0..12 {
        stone_world.set_block_world(x, 64, 1, Block::Stone);
    }
    let mut nav = Navigator::new(1, 0.25, 0.9);
    nav.update_goal_when_supported(Some(goal), start, &stone_world, true, &NavObstacles::none());
    assert_eq!(
        nav.path().last(),
        Some(&goal),
        "a one-block stone step stays routable: {:?}",
        nav.path()
    );

    let mut fence_world = flat_world();
    for x in 0..12 {
        fence_world.set_block_world(x, 64, 1, Block::OakFence);
    }
    let mut nav = Navigator::new(1, 0.25, 0.9);
    nav.update_goal_when_supported(Some(goal), start, &fence_world, true, &NavObstacles::none());
    assert_ne!(
        nav.path().last(),
        Some(&goal),
        "a one-high fence wall must not be routable: {:?}",
        nav.path()
    );
    assert!(
        nav.path().iter().all(|c| c.z < 1),
        "the best-effort route stops on the near side of the fence: {:?}",
        nav.path()
    );
}

#[test]
fn a_flight_of_stairs_routes_up() {
    use petramond_world::block_state::{StairHalf, StairState};
    let mut world = flat_world();
    let state = StairState::new(Facing::East, StairHalf::Bottom);
    assert!(world.place_stair(IVec3::new(5, 64, 1), Block::OakStairs, state));
    world.set_block_world(4, 64, 1, Block::Stone);
    assert!(world.place_stair(IVec3::new(4, 65, 1), Block::OakStairs, state));
    let top = IVec3::new(4, 66, 1);
    let route = route_probe(
        &world,
        crate::mob::Mob::Sheep,
        IVec3::new(7, 64, 1),
        top,
        &[],
        400,
    );
    assert_eq!(route, Some(Route::Open));
}

#[test]
fn a_climb_under_a_partial_shape_overhead_is_refused() {
    let mut world = flat_world();
    let (from, to) = (IVec3::new(4, 64, 1), IVec3::new(5, 65, 1));
    world.set_block_world(5, 64, 1, Block::Stone);
    let d = def(crate::mob::Mob::Sheep);
    let climbs = |world: &World| {
        navigation_step_gate(&world.cursor(), d.path_params(), d.size.height)(from, to)
    };
    assert!(climbs(&world));
    // Above the head where the body stands, clear of both standing poses.
    world.set_block_world(4, 66, 1, Block::GlassPane);
    assert!(!climbs(&world));
}

#[test]
fn a_climb_is_refused_where_the_real_rise_is_more_than_one_block() {
    // A chest top stands 1/8 below the foothold cell above it: one cell up
    // onto a full cube from there is a rise no jump makes.
    let mut world = flat_world();
    world.set_block_world(4, 64, 1, Block::Chest);
    world.set_block_world(5, 64, 1, Block::Stone);
    let (from, to) = (IVec3::new(4, 65, 1), IVec3::new(5, 66, 1));
    let d = def(crate::mob::Mob::Sheep);
    let climbs = |world: &World| {
        navigation_step_gate(&world.cursor(), d.path_params(), d.size.height)(from, to)
    };
    world.set_block_world(5, 65, 1, Block::Stone);
    assert!(!climbs(&world));
    // The same climb onto another chest top rises exactly one block.
    world.set_block_world(5, 65, 1, Block::Chest);
    assert!(climbs(&world));
}

#[test]
fn a_route_probe_cut_short_is_undecided_never_closed() {
    // A worker planning walls reads Closed as "this wall seals a room"; a
    // long detour the node cap cut short must not look like one.
    let mut world = flat_world();
    for x in 0..11 {
        world.set_block_world(x, 64, 1, Block::Stone);
        world.set_block_world(x, 65, 1, Block::Stone);
    }
    let (from, to) = (IVec3::new(0, 64, 0), IVec3::new(0, 64, 2));
    let probe =
        |world: &World, nodes| route_probe(world, crate::mob::Mob::Sheep, from, to, &[], nodes);
    assert_eq!(probe(&world, 4), Some(Route::Undecided));
    assert_eq!(probe(&world, 400), Some(Route::Open));
    world.set_block_world(11, 64, 1, Block::Stone);
    world.set_block_world(11, 65, 1, Block::Stone);
    assert_eq!(probe(&world, 400), Some(Route::Closed));
}

#[test]
fn a_walk_region_agrees_with_a_route_probe_for_every_cell_of_its_box() {
    // Ledges, a stair flight, a slab, a fence and a one-way drop: what a
    // flood memoizes or works out in reverse must stay what a probe walks.
    let mut world = flat_world();
    for x in 0..12 {
        for z in 4..6 {
            world.set_block_world(x, 63, z, Block::Grass);
        }
    }
    for x in 4..8 {
        for z in 0..5 {
            for y in 64..64 + (x - 3).min(3) {
                world.set_block_world(x, y, z, Block::Stone);
            }
        }
    }
    world.set_block_world(2, 64, 2, Block::OakSlab);
    world.set_block_world(9, 64, 1, Block::OakFence);
    world.set_block_world(9, 64, 2, Block::OakFence);
    world.set_block_world(10, 64, 4, Block::Stone);
    world.set_block_world(10, 65, 4, Block::Stone);
    world.set_block_world(10, 66, 4, Block::Stone);
    let kind = crate::mob::Mob::Sheep;
    let (min, max) = (IVec3::new(0, 62, 0), IVec3::new(11, 69, 5));
    let start = IVec3::new(1, 64, 1);
    let agrees = |world: &World, blocked: &[IVec3], toward: bool| {
        world.route_probe_budget().refill();
        let ask = FloodAsk {
            from: start,
            span: (min, max),
            toward,
            blocked,
            max_nodes: 4000,
        };
        let flood = match walk_region(world, kind, ask) {
            mod_api::Flood::Reached(cells) => cells,
            other => panic!("the box floods: {other:?}"),
        };
        let reached: std::collections::HashSet<[i32; 3]> = flood.iter().map(|(c, _)| *c).collect();
        assert!(reached.len() > 40, "the flood covers the box's ground");
        for x in min.x..=max.x {
            for y in min.y..=max.y {
                for z in min.z..=max.z {
                    let cell = IVec3::new(x, y, z);
                    let (a, b) = if toward { (cell, start) } else { (start, cell) };
                    // A probe inside the box searches what the floods keep:
                    // the same verdict for the same expansions as one that
                    // reads the world afresh.
                    let probe = |world: &World| {
                        world.route_probe_budget().refill();
                        let route = route_probe(world, kind, a, b, blocked, 4000);
                        (route, world.route_probe_budget().remaining())
                    };
                    let over_kept = probe(world);
                    let kept = world.kept_boxes().take();
                    let afresh = probe(world);
                    world.kept_boxes().put(kept);
                    assert_eq!(over_kept, afresh, "{a} to {b} (blocked {blocked:?})");
                    assert_eq!(
                        reached.contains(&cell.to_array()),
                        afresh.0 == Some(Route::Open),
                        "{cell} (toward {toward}, blocked {blocked:?})"
                    );
                }
            }
        }
    };
    // What one flood learned of the cells is kept for the next: a planned
    // block, then real changes (a wall raised on the way, a fence gone, a
    // floor block dug) must each be seen by the floods after them.
    for toward in [false, true] {
        agrees(&world, &[], toward);
        agrees(
            &world,
            &[IVec3::new(3, 64, 3), IVec3::new(3, 65, 3)],
            toward,
        );
        agrees(&world, &[], toward);
    }
    for z in 0..5 {
        world.set_block_world(3, 64, z, Block::Stone);
        world.set_block_world(3, 65, z, Block::Stone);
    }
    agrees(&world, &[], true);
    world.set_block_world(9, 64, 1, Block::Air);
    world.set_block_world(8, 63, 5, Block::Air);
    agrees(&world, &[IVec3::new(9, 64, 2)], false);
    agrees(&world, &[], true);
}

#[test]
fn a_walk_region_floods_one_way_drops_by_direction() {
    // A ledge two blocks up: a body drops off it but cannot climb onto it,
    // so the ground is reached FROM the ledge and never walks TO it.
    let mut world = flat_world();
    for x in 5..9 {
        for z in 0..4 {
            world.set_block_world(x, 64, z, Block::Stone);
            world.set_block_world(x, 65, z, Block::Stone);
        }
    }
    let (ledge, ground) = (IVec3::new(6, 66, 1), IVec3::new(1, 64, 1));
    let (min, max) = (IVec3::new(0, 60, 0), IVec3::new(12, 70, 3));
    let flood = |toward, nodes| {
        walk_region(
            &world,
            crate::mob::Mob::Sheep,
            FloodAsk {
                from: ledge,
                span: (min, max),
                toward,
                blocked: &[],
                max_nodes: nodes,
            },
        )
    };
    let reached = |toward| match flood(toward, 4000) {
        mod_api::Flood::Reached(cells) => cells.into_iter().map(|(c, _)| c).collect::<Vec<_>>(),
        other => panic!("the box floods: {other:?}"),
    };
    let from_ledge = reached(false);
    assert!(
        from_ledge.contains(&ground.to_array()),
        "the ground is reached by dropping off"
    );
    let moves = |cell: [i32; 3]| match flood(false, 4000) {
        mod_api::Flood::Reached(cells) => {
            cells.into_iter().find(|(c, _)| *c == cell).map(|(_, m)| m)
        }
        other => panic!("the box floods: {other:?}"),
    };
    assert_eq!(
        (moves(ledge.to_array()), moves([8, 66, 1])),
        (Some(0), Some(2)),
        "each foothold carries its moves from the start"
    );
    let to_ledge = reached(true);
    assert!(
        to_ledge.contains(&[8, 66, 3]),
        "the ledge's own top walks to it"
    );
    assert!(
        !to_ledge.contains(&ground.to_array()),
        "the ground never climbs two blocks"
    );
    assert_eq!(
        flood(false, 8),
        mod_api::Flood::Exceeded,
        "more ground than asked for"
    );
}

#[test]
fn mob_can_reach_answers_the_fence_honestly() {
    // The `MobCanReach` HostCall's engine seam: a cell beyond a fence
    // line is not reachable, a cell on the mob's own side is — the
    // honesty gate mod destination policies (grazing) build on.
    let mut world = flat_world();
    for x in 0..12 {
        world.set_block_world(x, 64, 1, Block::OakFence);
    }
    let mob = crate::mob::Instance::new(
        crate::mob::Mob::Sheep,
        WorldPos::new(4.5, 64.0, 0.5),
        0.0,
        1,
    );
    assert!(
        !mob_can_reach(&world, &mob, IVec3::new(4, 64, 2)),
        "grass beyond the fence is not a reachable destination"
    );
    assert!(
        mob_can_reach(&world, &mob, IVec3::new(8, 64, 0)),
        "a cell on the mob's own side is reachable"
    );
}

#[test]
fn a_step_beside_the_fence_opens_the_route_over_it() {
    // The pen rule's honest exception: with a block placed in front of the
    // fence, the mob may jump onto it and walk over the fence top.
    let mut world = flat_world();
    for x in 0..12 {
        world.set_block_world(x, 64, 1, Block::OakFence);
    }
    world.set_block_world(4, 64, 0, Block::Dirt);
    // The mob starts beside the step on the ground, goal beyond the fence.
    let start = IVec3::new(3, 64, 0);
    let goal = IVec3::new(4, 64, 2);
    let mut nav = Navigator::new(1, 0.25, 0.9);
    nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
    assert_eq!(
        nav.path().last(),
        Some(&goal),
        "the route should go over the fence via the step: {:?}",
        nav.path()
    );
    assert!(
        nav.path().contains(&IVec3::new(4, 65, 1)),
        "the route walks the fence top: {:?}",
        nav.path()
    );
}

#[test]
fn an_offset_body_deflects_along_a_partial_shapes_face_instead_of_pressing_in() {
    // The walking-against-the-trough bug: a sheep that wandered flush
    // against a partial-collision block (here a chest) gets a waypoint
    // past it. The raw wish from its OFFSET position presses the body
    // diagonally into the shape; steered following must drop the blocked
    // axis and walk cleanly along the face instead.
    let mut world = flat_world();
    world.set_block_world(4, 64, 1, Block::Chest);
    let mut nav = Navigator::new(2, 0.45, 1.4);
    nav.path = vec![IVec3::new(3, 64, 0), IVec3::new(5, 64, 0)];
    nav.index = 1;
    nav.goal = Some(IVec3::new(5, 64, 0));
    nav.path_reaches_goal = true;
    // Body centre at z = 0.95: its 0.9-wide body overlaps the chest's
    // row, so heading straight east grinds into the chest's west face.
    let pos = WorldPos::new(3.5, 64.0, 0.95);

    let (raw, _) = nav.follow(pos, true);
    assert!(
        raw.x > 0.9,
        "the raw wish presses east into the chest: {raw:?}"
    );

    let mut nav = Navigator::new(2, 0.45, 1.4);
    nav.path = vec![IVec3::new(3, 64, 0), IVec3::new(5, 64, 0)];
    nav.index = 1;
    nav.goal = Some(IVec3::new(5, 64, 0));
    nav.path_reaches_goal = true;
    let (wish, jump) = nav.follow_steered(pos, true, &world);
    assert!(!jump);
    assert!(
        wish.x.abs() < 0.05 && wish.z < -0.9,
        "steering drops the blocked axis and clears the chest's row first: {wish:?}"
    );
}

#[test]
fn steering_never_deflects_the_final_approach_to_a_wall_adjacent_waypoint() {
    // The probe is capped at the remaining distance: a waypoint whose far
    // side is a wall must still be walked INTO the arrive window, or mobs
    // hover forever one body-length short of wall-adjacent destinations.
    let mut world = flat_world();
    world.set_block_world(5, 64, 0, Block::Stone);
    world.set_block_world(5, 65, 0, Block::Stone);
    let mut nav = Navigator::new(2, 0.45, 1.4);
    nav.path = vec![IVec3::new(3, 64, 0), IVec3::new(4, 64, 0)];
    nav.index = 1;
    nav.goal = Some(IVec3::new(4, 64, 0));
    nav.path_reaches_goal = true;
    let (wish, _) = nav.follow_steered(WorldPos::new(4.42, 64.0, 0.5), true, &world);
    assert!(
        wish.x > 0.9,
        "the final approach keeps closing on the wall-adjacent centre: {wish:?}"
    );
}

#[test]
fn no_diagonal_step_cuts_past_a_partial_shapes_corner() {
    // The gate's sweep is axis-ordered, but a mob walks a diagonal as a
    // straight line — which can clip a partial shape's corner the L-shaped
    // sweep cleared. Diagonals near body-level partial shapes are refused
    // outright, so the route around a trough/chest is honest cardinal
    // steps a real body can walk.
    let mut world = flat_world();
    world.set_block_world(4, 64, 1, Block::Chest);
    let start = IVec3::new(3, 64, 1);
    let goal = IVec3::new(4, 64, 0);
    let mut nav = Navigator::new(2, 0.45, 1.4);
    nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
    assert_eq!(nav.path().last(), Some(&goal), "the goal stays reachable");
    assert!(
        nav.path()
            .windows(2)
            .all(|w| (w[1].x - w[0].x).abs() + (w[1].z - w[0].z).abs() <= 1),
        "no diagonal step beside the chest — cardinal detour only: {:?}",
        nav.path()
    );
}

#[test]
fn routes_bend_around_another_mobs_body() {
    let world = flat_world();
    let start = IVec3::new(1, 64, 1);
    let goal = IVec3::new(8, 64, 1);
    let blocker_cell = IVec3::new(4, 64, 1);
    let mobs = [AiMob {
        id: 7,
        kind: crate::mob::Mob::Sheep,
        pos: WorldPos::new(4.5, 64.0, 1.5),
        active: true,
        tags: Default::default(),
    }];
    let obstacles = NavObstacles {
        self_id: 1,
        target: None,
        mobs: &mobs,
        players: &[],
    };
    let mut nav = Navigator::new(1, 0.25, 0.9);
    nav.update_goal_when_supported(Some(goal), start, &world, true, &obstacles);
    assert_eq!(nav.path().last(), Some(&goal), "still reaches the goal");
    assert!(
        !nav.path().contains(&blocker_cell),
        "the route bends around the standing mob: {:?}",
        nav.path()
    );
}

#[test]
fn a_blocked_corridor_is_rounded_rather_than_pushed_through() {
    // A 1-wide corridor with a sheep standing in it and a gap in one wall
    // before AND after her: squeezing past her body costs one crowded cell
    // (200), the gap detour costs a handful of flat steps (~40). The route
    // must take the gap, not the shove.
    let mut world = flat_world();
    for x in 0..12 {
        if x != 3 && x != 5 {
            world.set_block_world(x, 64, 0, Block::Stone);
        }
        world.set_block_world(x, 64, 2, Block::Stone);
    }
    let start = IVec3::new(1, 64, 1);
    let goal = IVec3::new(8, 64, 1);
    let blocker_cell = IVec3::new(4, 64, 1);
    let mobs = [AiMob {
        id: 7,
        kind: crate::mob::Mob::Sheep,
        pos: WorldPos::new(4.5, 64.0, 1.5),
        active: true,
        tags: Default::default(),
    }];
    let obstacles = NavObstacles {
        self_id: 1,
        target: None,
        mobs: &mobs,
        players: &[],
    };
    let mut nav = Navigator::new(1, 0.25, 0.9);
    nav.update_goal_when_supported(Some(goal), start, &world, true, &obstacles);
    assert_eq!(nav.path().last(), Some(&goal), "still reaches the goal");
    assert!(
        !nav.path().contains(&blocker_cell),
        "the route rounds the corridor through the gaps: {:?}",
        nav.path()
    );
}

#[test]
fn a_truly_blocked_crowd_still_resolves_by_paying_the_cost() {
    // The other half of the contract: no detour at all (sealed corridor)
    // must never deadlock the search — the mob squeezes through as a last
    // resort instead of freezing.
    let mut world = flat_world();
    for x in 0..12 {
        world.set_block_world(x, 64, 0, Block::Stone);
        world.set_block_world(x, 64, 2, Block::Stone);
    }
    let start = IVec3::new(1, 64, 1);
    let goal = IVec3::new(8, 64, 1);
    let mobs = [AiMob {
        id: 7,
        kind: crate::mob::Mob::Sheep,
        pos: WorldPos::new(4.5, 64.0, 1.5),
        active: true,
        tags: Default::default(),
    }];
    let obstacles = NavObstacles {
        self_id: 1,
        target: None,
        mobs: &mobs,
        players: &[],
    };
    let mut nav = Navigator::new(1, 0.25, 0.9);
    nav.update_goal_when_supported(Some(goal), start, &world, true, &obstacles);
    assert_eq!(
        nav.path().last(),
        Some(&goal),
        "a packed corridor still resolves by squeezing through: {:?}",
        nav.path()
    );
}

#[test]
fn the_locked_target_is_never_avoided() {
    // A zombie chasing prey must path TO it, not around it: the current
    // target is exempt from entity avoidance.
    let world = flat_world();
    let start = IVec3::new(1, 64, 1);
    let goal = IVec3::new(8, 64, 1);
    let on_the_way = IVec3::new(4, 64, 1);
    let mobs = [AiMob {
        id: 7,
        kind: crate::mob::Mob::Sheep,
        pos: WorldPos::new(4.5, 64.0, 1.5),
        active: true,
        tags: Default::default(),
    }];
    let obstacles = NavObstacles {
        self_id: 1,
        target: Some(EntityRef::Mob(7)),
        mobs: &mobs,
        players: &[],
    };
    let mut nav = Navigator::new(1, 0.25, 0.9);
    nav.update_goal_when_supported(Some(goal), start, &world, true, &obstacles);
    assert!(
        nav.path().contains(&on_the_way),
        "the targeted mob's cells cost nothing extra: {:?}",
        nav.path()
    );
}

#[test]
fn players_are_soft_obstacles_unless_targeted() {
    let world = flat_world();
    let start = IVec3::new(1, 64, 1);
    let goal = IVec3::new(8, 64, 1);
    let player_cell = IVec3::new(4, 64, 1);
    let anchor = PlayerAnchor {
        body: Some(petramond_world::body::Body::new(
            WorldPos::new(4.5, 64.0, 1.5),
            0.3,
            1.8,
        )),
        ..Default::default()
    };
    let players = [anchor];

    let mut nav = Navigator::new(1, 0.25, 0.9);
    let avoid = NavObstacles {
        self_id: 1,
        target: None,
        mobs: &[],
        players: &players,
    };
    nav.update_goal_when_supported(Some(goal), start, &world, true, &avoid);
    assert_eq!(nav.path().last(), Some(&goal));
    assert!(
        !nav.path().contains(&player_cell),
        "an untargeted player is routed around: {:?}",
        nav.path()
    );

    let mut nav = Navigator::new(1, 0.25, 0.9);
    let chase = NavObstacles {
        self_id: 1,
        target: Some(EntityRef::Player(players[0].id)),
        mobs: &[],
        players: &players,
    };
    nav.update_goal_when_supported(Some(goal), start, &world, true, &chase);
    assert!(
        nav.path().contains(&player_cell),
        "the hunted player is pathed TO, not around: {:?}",
        nav.path()
    );
}

#[test]
fn re_paths_a_held_goal_when_the_world_changes() {
    // Hold one goal while the world changes underneath: the navigator must keep the
    // first route until REPATH_TICKS elapse, then refresh it to route around new
    // terrain — the whole point of periodic re-pathing.
    let mut world = flat_world();
    let start = IVec3::new(1, 64, 1);
    let goal = IVec3::new(8, 64, 1);
    let mut nav = Navigator::new(1, 0.25, 0.9);

    // Initial path: a straight run along z = 1, passing through (4, 64, 1).
    nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
    let stale: Vec<IVec3> = nav.path().to_vec();
    let blocked = IVec3::new(4, 64, 1);
    assert!(
        stale.contains(&blocked),
        "the open route runs straight through {blocked:?}"
    );

    // Drop a 2-high wall across that cell (its foothold + the cell above it), so the
    // straight route is no longer walkable — a detour via z = 0 / z = 2 remains.
    world.set_block_world(blocked.x, blocked.y, blocked.z, Block::Stone);
    world.set_block_world(blocked.x, blocked.y + 1, blocked.z, Block::Stone);

    // For the first REPATH_TICKS-1 held ticks the stale route is kept verbatim (no
    // per-tick re-pathing — holding a goal stays cheap).
    for _ in 0..REPATH_TICKS - 1 {
        nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
        assert_eq!(
            nav.path(),
            stale.as_slice(),
            "no re-path before the interval elapses"
        );
    }

    // The REPATH_TICKS-th held tick refreshes the route, which now avoids the wall.
    nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
    assert_ne!(
        nav.path(),
        stale.as_slice(),
        "re-paths once the interval elapses"
    );
    assert!(
        !nav.path().contains(&blocked),
        "the refreshed route avoids the newly-walled cell: {:?}",
        nav.path()
    );
}

#[test]
fn fast_wide_mob_consumes_overflown_waypoints_instead_of_orbiting() {
    // The hushjaw jitter regression: half_width 0.45 tightens arrive_xz to
    // 0.05 m while 4.8 m/s walks 0.24 m per tick — the mob can overfly a
    // waypoint it can never land inside. Overshoot consumption must let it
    // walk a straight route with ZERO 180° turn-backs; without it this
    // exact setup orbits the first waypoint forever (measured: 1990
    // reversals in 2000 ticks).
    let mut nav = Navigator::new(2, 0.45, 1.4);
    nav.path = (0..=8).map(|x| IVec3::new(x, 1, 0)).collect();
    nav.index = 1;
    nav.goal = Some(IVec3::new(8, 1, 0));
    nav.path_reaches_goal = true;

    let step = 4.8 * 0.05; // hushjaw speed × tick dt
    let mut pos = WorldPos::new(0.5, 1.0, 0.5);
    let mut last_dir: Option<Vec3> = None;
    let mut reversals = 0;
    for _ in 0..200 {
        let (wish, _jump) = nav.follow(pos, true);
        if wish == Vec3::ZERO {
            break;
        }
        if let Some(prev) = last_dir {
            if wish.x * prev.x + wish.z * prev.z < -0.5 {
                reversals += 1;
            }
        }
        last_dir = Some(wish);
        pos += wish * step;
    }

    assert!(
        nav.is_idle(),
        "the straight 8-cell route completes within the tick budget"
    );
    assert_eq!(
        reversals, 0,
        "no 180° turn-backs while following a straight route at speed"
    );
}

#[test]
fn overshoot_does_not_consume_a_waypoint_still_being_approached() {
    // The wide-corner contract's counterpart: while the distance to the
    // waypoint is still SHRINKING, overshoot must not fire — a wide mob
    // keeps clearing the corner exactly as before.
    let mut nav = Navigator::new(1, 0.45, 0.9);
    nav.path = vec![
        IVec3::new(0, 1, 0),
        IVec3::new(1, 1, 0),
        IVec3::new(1, 1, 1),
    ];
    nav.index = 1;
    nav.goal = Some(IVec3::new(1, 1, 1));
    // Two approaching ticks toward the corner waypoint (1,1,0): both must
    // keep steering east at it, not consume it early.
    for x in [1.1_f32, 1.25] {
        let (wish, _) = nav.follow(WorldPos::new(f64::from(x), 1.0, 0.5), true);
        assert!(
            wish.x > 0.9 && wish.z.abs() < 0.1,
            "still clearing the corner at x={x}: {wish:?}"
        );
    }
}

#[test]
fn changed_goal_repath_preserves_the_current_waypoint_when_still_valid() {
    // A chased target crossing a cell boundary CHANGES the goal several
    // times a second. The refresh must keep steering at the waypoint the
    // mob is mid-stride toward (when it remains a valid immediate step),
    // not snap laterally between equal-cost first steps.
    let world = flat_world();
    let start = IVec3::new(1, 64, 1);
    let old_waypoint = IVec3::new(2, 64, 1);
    let first_goal = IVec3::new(2, 64, 3);
    let moved_goal = IVec3::new(3, 64, 3);

    let mut nav = Navigator::new(1, 0.25, 0.9);
    nav.path = vec![start, old_waypoint, IVec3::new(2, 64, 2), first_goal];
    nav.index = 1;
    nav.goal = Some(first_goal);
    nav.path_reaches_goal = true;

    nav.update_goal_when_supported(Some(moved_goal), start, &world, true, &NavObstacles::none());
    assert_eq!(
        nav.path().get(1),
        Some(&old_waypoint),
        "a changed goal keeps the in-progress step when still valid"
    );
    assert_eq!(
        nav.path().last(),
        Some(&moved_goal),
        "and the preserved route still reaches the new goal"
    );
}

#[test]
fn a_goal_at_or_behind_the_mob_never_stitches_a_u_turn_through_the_kept_waypoint() {
    // The hold pattern (a node emitting the mob's OWN cell to stand still)
    // and a target passing the mob both move the goal to/behind the start.
    // Preserving the in-progress waypoint there builds start → wp → start,
    // whose return leg steers the body straight back — the every-tick
    // out-and-back shaking at a lure. The kept step must be dropped.
    let world = flat_world();
    let start = IVec3::new(3, 64, 3);
    let old_waypoint = IVec3::new(4, 64, 4);
    let first_goal = IVec3::new(6, 64, 6);

    let mut nav = Navigator::new(1, 0.45, 1.3);
    nav.path = vec![start, old_waypoint, IVec3::new(5, 64, 5), first_goal];
    nav.index = 1;
    nav.goal = Some(first_goal);
    nav.path_reaches_goal = true;

    // Goal = own cell: a hold, not a route.
    nav.update_goal_when_supported(Some(start), start, &world, true, &NavObstacles::none());
    assert!(nav.is_idle(), "holding position is arrival, never a detour");
    assert!(
        !nav.path().contains(&old_waypoint),
        "no out-and-back through the old waypoint: {:?}",
        nav.path()
    );

    // Goal behind the mob: the direct route, whose first step is not the
    // old (now hairpin) waypoint.
    let mut nav = Navigator::new(1, 0.45, 1.3);
    nav.path = vec![start, old_waypoint, IVec3::new(5, 64, 5), first_goal];
    nav.index = 1;
    nav.goal = Some(first_goal);
    nav.path_reaches_goal = true;
    let behind = IVec3::new(1, 64, 1);
    nav.update_goal_when_supported(Some(behind), start, &world, true, &NavObstacles::none());
    assert_ne!(nav.path().get(1), Some(&old_waypoint));
    assert!(!nav.path()[1..].contains(&start));
    assert_eq!(nav.path().last(), Some(&behind));
}

#[test]
fn same_goal_repath_preserves_the_current_waypoint_when_still_valid() {
    // A periodic refresh can find an equally-good path whose first step is a
    // different lateral cell. The mob should not snap sideways on the refresh tick
    // if the waypoint it was already walking toward is still a valid immediate step.
    let world = flat_world();
    let start = IVec3::new(1, 64, 1);
    let old_waypoint = IVec3::new(2, 64, 1);
    let goal = IVec3::new(2, 64, 3);
    let params = PathParams::for_body(1, 0.25);
    let direct = path::find_path(
        start,
        goal,
        params,
        |c| world.blocks_movement_at(c.x, c.y, c.z),
        |c| world.fluid_cell_at(c.x, c.y, c.z),
    );
    assert_ne!(
        direct.get(1),
        Some(&old_waypoint),
        "fixture must have a direct refresh route that would pick a different first step"
    );

    let mut nav = Navigator::new(1, 0.25, 0.9);
    nav.path = vec![start, old_waypoint, IVec3::new(2, 64, 2), goal];
    nav.index = 1;
    nav.goal = Some(goal);
    nav.path_reaches_goal = true;
    nav.since_path = REPATH_TICKS - 1;

    nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
    assert_eq!(
        nav.path().get(1),
        Some(&old_waypoint),
        "same-goal refresh keeps steering toward the current valid waypoint"
    );
}

#[test]
fn unreachable_goal_backs_off_consecutive_same_goal_repaths() {
    let (world, start, goal) = pillar_world();
    let mut nav = Navigator::new(1, 0.25, 0.9);

    nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
    assert_eq!(nav.recomputes(), 1);
    assert_ne!(
        nav.path().last(),
        Some(&goal),
        "the pillar top is unreachable, so the route is partial"
    );

    for _ in 0..REPATH_TICKS - 1 {
        nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
    }
    assert_eq!(nav.recomputes(), 1, "no retry before the base interval");

    nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
    assert_eq!(
        nav.recomputes(),
        2,
        "first same-goal retry happens at the base interval"
    );

    for _ in 0..(REPATH_TICKS * 2 - 1) {
        nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
    }
    assert_eq!(
        nav.recomputes(),
        2,
        "a second failed retry waits for the doubled backoff interval"
    );

    nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
    assert_eq!(
        nav.recomputes(),
        3,
        "the doubled interval eventually permits a retry"
    );
}

#[test]
fn goal_cell_change_resets_unreachable_backoff_immediately() {
    let (world, start, unreachable) = pillar_world();
    let reachable = IVec3::new(2, 64, 1);
    let mut nav = Navigator::new(1, 0.25, 0.9);

    nav.update_goal_when_supported(
        Some(unreachable),
        start,
        &world,
        true,
        &NavObstacles::none(),
    );
    for _ in 0..REPATH_TICKS {
        nav.update_goal_when_supported(
            Some(unreachable),
            start,
            &world,
            true,
            &NavObstacles::none(),
        );
    }
    assert_eq!(
        nav.recomputes(),
        2,
        "the unreachable goal has entered backoff"
    );

    nav.update_goal_when_supported(Some(reachable), start, &world, true, &NavObstacles::none());
    assert_eq!(
        nav.recomputes(),
        3,
        "a different goal cell is pathed immediately despite the old goal's backoff"
    );
    assert_eq!(
        nav.path().last(),
        Some(&reachable),
        "the changed reachable goal gets a complete route"
    );
}

#[test]
fn reachable_goal_keeps_the_base_repath_interval() {
    let world = flat_world();
    let start = IVec3::new(1, 64, 1);
    let goal = IVec3::new(8, 64, 1);
    let mut nav = Navigator::new(1, 0.25, 0.9);

    nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
    assert_eq!(nav.recomputes(), 1);
    assert_eq!(nav.path().last(), Some(&goal));

    for expected in 2..=3 {
        for _ in 0..REPATH_TICKS - 1 {
            nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
        }
        assert_eq!(
            nav.recomputes(),
            expected - 1,
            "no early reachable-goal repath"
        );
        nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
        assert_eq!(
            nav.recomputes(),
            expected,
            "reachable held goals keep the normal cadence"
        );
    }
}

#[test]
fn held_goal_does_not_repath_while_airborne() {
    let mut world = flat_world();
    let start = IVec3::new(1, 64, 1);
    let goal = IVec3::new(8, 64, 1);
    let mut nav = Navigator::new(1, 0.25, 0.9);
    nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
    let stale: Vec<IVec3> = nav.path().to_vec();
    let blocked = IVec3::new(4, 64, 1);
    assert!(stale.contains(&blocked));

    world.set_block_world(blocked.x, blocked.y, blocked.z, Block::Stone);
    world.set_block_world(blocked.x, blocked.y + 1, blocked.z, Block::Stone);

    for _ in 0..REPATH_TICKS - 1 {
        nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
    }
    assert_eq!(nav.path(), stale.as_slice());

    for _ in 0..5 {
        nav.update_goal_when_supported(Some(goal), start, &world, false, &NavObstacles::none());
        assert_eq!(nav.path(), stale.as_slice(), "mid-air repath is paused");
    }

    nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
    assert_ne!(
        nav.path(),
        stale.as_slice(),
        "repath resumes once the mob is supported again"
    );
}

#[test]
fn re_path_does_not_reset_the_stuck_tally() {
    // A mob that never moves stays stuck across re-paths: the stuck counter must keep
    // climbing through a refresh (not reset to zero each interval), so a wedged mob
    // still abandons its goal instead of re-pathing into the same wall forever.
    let world = flat_world();
    let start = IVec3::new(1, 64, 1);
    let goal = IVec3::new(8, 64, 1);
    let mut nav = Navigator::new(1, 0.25, 0.9);
    nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());

    // Drive enough held ticks to cross several re-path intervals AND the stuck limit,
    // following from a fixed position each tick so no progress is ever made.
    let wedged = WorldPos::new(1.5, 64.0, 1.5);
    let mut gave_up = false;
    for _ in 0..STUCK_TICKS + REPATH_TICKS {
        nav.update_goal_when_supported(Some(goal), start, &world, true, &NavObstacles::none());
        nav.follow(wedged, true);
        if nav.is_idle() {
            gave_up = true;
            break;
        }
    }
    assert!(
        gave_up,
        "a permanently-wedged mob still abandons its goal despite re-pathing"
    );
}
