//! The navigator: turns a destination cell into per-tick locomotion.
//!
//! Given a goal from the [brain](super::brain), it pathfinds (re-pathing when the goal
//! changes, and otherwise every [`REPATH_TICKS`] to refresh a stale route), then each
//! tick steers the mob toward the next foothold on the path — jumping when it reaches a
//! one-block step up, walking off ledges to descend. Waypoints are consumed as the mob
//! reaches them; if the mob stops making progress (wedged against geometry) the path is
//! abandoned so the brain can pick a new goal instead of pushing into a wall forever.

use crate::mathh::{IVec3, Vec3};
use crate::world::World;

use super::path::{self, PathParams};

/// Largest horizontal distance (m) within which a waypoint counts as reached. The
/// actual threshold tightens for wide mobs so they don't turn before their body has
/// cleared a corner.
const MAX_ARRIVE_XZ: f32 = 0.3;
/// Never require perfect centre hits; discrete tick movement can step over a waypoint
/// by a few centimetres.
const MIN_ARRIVE_XZ: f32 = 0.04;
/// Vertical distance (m) within which the mob is "on the waypoint's level" — so a
/// descent waypoint isn't marked reached until the mob has actually fallen to it.
const ARRIVE_Y: f32 = 1.1;
/// Begin a jump once the body's leading edge is this close to the higher waypoint's
/// centre. The actual centre-distance threshold is `half_width + this`: wider mobs
/// reach a ledge with their body before their centre gets near it.
const JUMP_TRIGGER_FRONT_XZ: f32 = 0.7;
/// Ticks of negligible movement before the path is abandoned (~2 s at 20 TPS).
const STUCK_TICKS: u32 = 40;
/// Squared per-tick displacement below which the mob counts as "not progressing".
const STUCK_EPS_SQ: f32 = 0.015 * 0.015;
/// Re-pathfind toward an *unchanged* goal once this many ticks have passed since the
/// current path was computed — once a second at 20 TPS. Long enough that holding a goal
/// across ticks is cheap, short enough that a path computed against an earlier world
/// state is refreshed: it invalidates a route since blocked by terrain changes, and
/// picks up a now-shorter one, instead of a mob following a stale path forever. The
/// stuck tally (above) carries across the refresh, so a mob wedged the whole time still
/// eventually gives up rather than re-pathing into the same wall indefinitely.
const REPATH_TICKS: u32 = 20;
/// Longest same-goal retry interval after repeated partial/failed searches.
const MAX_REPATH_BACKOFF_TICKS: u32 = 200;

pub struct Navigator {
    path: Vec<IVec3>,
    /// Index of the next waypoint to walk to.
    index: usize,
    goal: Option<IVec3>,
    /// Whether the current path reaches `goal`. Failed searches still keep a
    /// best-effort partial path, but same-goal refreshes back off.
    path_reaches_goal: bool,
    params: PathParams,
    half_width: f32,
    stuck: u32,
    last_pos: Vec3,
    /// Ticks since the current path was computed; at [`REPATH_TICKS`] the held goal is
    /// re-pathed to refresh a route gone stale (see the constant).
    since_path: u32,
    /// Current same-goal retry interval. Successful routes and goal changes reset this
    /// to [`REPATH_TICKS`]; repeated partial searches double it up to the cap.
    repath_interval: u32,
    #[cfg(test)]
    recomputes: u32,
}

impl Navigator {
    pub fn new(head: i32, half_width: f32) -> Self {
        Navigator {
            path: Vec::new(),
            index: 0,
            goal: None,
            path_reaches_goal: false,
            params: PathParams::for_body(head, half_width),
            half_width,
            stuck: 0,
            last_pos: Vec3::ZERO,
            since_path: 0,
            repath_interval: REPATH_TICKS,
            #[cfg(test)]
            recomputes: 0,
        }
    }

    /// No active path — the mob has arrived, given up, or was never tasked. The
    /// brain reads this (via `AiCtx::nav_idle`) to know it may pick a new goal.
    pub fn is_idle(&self) -> bool {
        self.goal.is_none() || self.index >= self.path.len()
    }

    /// The current path (foothold cells, start→goal), for tests to observe re-pathing.
    #[cfg(test)]
    pub(super) fn path(&self) -> &[IVec3] {
        &self.path
    }

    #[cfg(test)]
    pub(super) fn recomputes(&self) -> u32 {
        self.recomputes
    }

    fn clear(&mut self) {
        self.path.clear();
        self.index = 0;
        self.goal = None;
        self.path_reaches_goal = false;
        self.stuck = 0;
        self.since_path = 0;
        self.repath_interval = REPATH_TICKS;
    }

    /// Set the navigation goal and keep the path fresh. A *new* goal is pathed at once
    /// (resetting progress + the stuck tally); the *same* goal held across ticks costs
    /// nothing until [`REPATH_TICKS`] elapse, then it is re-pathed to refresh a route
    /// the changing world may have invalidated or shortened. `None` clears the path.
    ///
    /// Periodic and new-goal pathfinding is paused while `can_repath` is false. A
    /// falling mob keeps following its existing route instead of recomputing from
    /// transient mid-air cells.
    pub fn update_goal_when_supported(
        &mut self,
        goal: Option<IVec3>,
        start: IVec3,
        world: &World,
        can_repath: bool,
    ) {
        match goal {
            None => {
                if self.goal.is_some() {
                    self.clear();
                }
            }
            Some(g) => {
                if !can_repath {
                    return;
                }
                if self.goal != Some(g) {
                    // A new goal: path to it afresh and reset the stuck tally — this is a
                    // deliberate new destination, not the same one re-evaluated. It also
                    // drops any unreachable-goal backoff from the previous cell.
                    self.repath_interval = REPATH_TICKS;
                    self.recompute(start, g, world, true);
                    self.goal = Some(g);
                    self.stuck = 0;
                } else {
                    // Same goal held: refresh the route at the current interval. A
                    // reachable route uses the normal cadence; repeated partial/failed
                    // routes stretch this interval to avoid exhausting A* every second for
                    // an unreachable target. The stuck tally is left to keep climbing
                    // across refreshes, so a mob wedged the whole time still abandons the
                    // goal rather than re-pathing forever.
                    self.since_path = self.since_path.saturating_add(1);
                    if self.since_path >= self.repath_interval {
                        self.recompute(start, g, world, false);
                    }
                }
            }
        }
    }

    /// (Re)compute the path from `start` to `goal`, resetting the waypoint cursor to the
    /// first step and the repath timer. Shared by a goal change and the periodic refresh.
    fn recompute(&mut self, start: IVec3, goal: IVec3, world: &World, goal_changed: bool) {
        let solid = navigation_solid_fn(world);
        let water = |c: IVec3| world.water_cell_at(c.x, c.y, c.z);
        let step_allowed = door_step_gate(world, self.params);
        let old_waypoint =
            (!goal_changed && self.index < self.path.len()).then(|| self.path[self.index]);
        self.path = old_waypoint
            .and_then(|wp| {
                preserve_waypoint_path(start, wp, goal, self.params, &solid, &water, &step_allowed)
            })
            .unwrap_or_else(|| {
                path::find_path_with_step_gate(
                    start,
                    goal,
                    self.params,
                    &solid,
                    water,
                    step_allowed,
                )
            });
        self.path_reaches_goal = self.path.last().is_some_and(|&last| last == goal);
        // Index 1 = the first cell to walk to (path[0] is the start).
        self.index = if self.path.len() > 1 {
            1
        } else {
            self.path.len()
        };
        self.since_path = 0;
        if self.path_reaches_goal || goal_changed {
            self.repath_interval = REPATH_TICKS;
        } else {
            self.repath_interval = next_repath_backoff(self.repath_interval);
        }
        #[cfg(test)]
        {
            self.recomputes += 1;
        }
    }

    /// This tick's locomotion: a unit horizontal `wish` direction toward the current
    /// waypoint, and whether to jump. Consumes waypoints as they're reached and
    /// abandons the path if the mob stalls.
    pub fn follow(&mut self, pos: Vec3, on_ground: bool) -> (Vec3, bool) {
        let arrive_xz = self.arrive_xz();
        while self.index < self.path.len() {
            let wp = self.path[self.index];
            let (tx, tz) = (wp.x as f32 + 0.5, wp.z as f32 + 0.5);
            let (dx, dz) = (tx - pos.x, tz - pos.z);
            let horiz = (dx * dx + dz * dz).sqrt();
            let dy = (pos.y - wp.y as f32).abs();

            if horiz <= arrive_xz && dy <= ARRIVE_Y {
                self.index += 1;
                continue; // reached this waypoint; aim at the next
            }

            // Progress / stuck tracking.
            let progress_dx = pos.x - self.last_pos.x;
            let progress_dz = pos.z - self.last_pos.z;
            if progress_dx * progress_dx + progress_dz * progress_dz < STUCK_EPS_SQ {
                self.stuck += 1;
            } else {
                self.stuck = 0;
            }
            self.last_pos = pos;
            if self.stuck >= STUCK_TICKS {
                self.clear();
                return (Vec3::ZERO, false);
            }

            let dir = if horiz > 1e-4 {
                Vec3::new(dx / horiz, 0.0, dz / horiz)
            } else {
                Vec3::ZERO
            };
            // Jump when the next waypoint is a step up and we're grounded + close to
            // the edge, so forward speed carries the mob onto the higher block.
            let step_up = wp.y as f32 > pos.y + 0.5;
            let jump = on_ground && step_up && horiz <= self.half_width + JUMP_TRIGGER_FRONT_XZ;
            return (dir, jump);
        }

        // Path exhausted. A route that reached the goal has arrived and resets the
        // navigator. A partial route stays associated with the same goal so held-goal
        // retries obey the unreachable-goal backoff instead of immediately recomputing.
        if self.path_reaches_goal {
            self.clear();
        } else {
            self.path.clear();
            self.index = 0;
        }
        (Vec3::ZERO, false)
    }

    fn arrive_xz(&self) -> f32 {
        MAX_ARRIVE_XZ.min((0.5 - self.half_width).max(MIN_ARRIVE_XZ))
    }
}

fn preserve_waypoint_path(
    start: IVec3,
    waypoint: IVec3,
    goal: IVec3,
    params: PathParams,
    solid: &impl Fn(IVec3) -> bool,
    water: &impl Fn(IVec3) -> bool,
    step_allowed: &impl Fn(IVec3, IVec3) -> bool,
) -> Option<Vec<IVec3>> {
    if waypoint == start {
        return None;
    }
    let step = path::find_path_with_step_gate(start, waypoint, params, solid, water, step_allowed);
    if step.last() != Some(&waypoint) || step.len() > 2 {
        return None;
    }
    if waypoint == goal {
        return Some(step);
    }
    let suffix = path::find_path_with_step_gate(waypoint, goal, params, solid, water, step_allowed);
    if suffix.first() != Some(&waypoint) || suffix.len() <= 1 {
        return None;
    }
    let mut stitched = step;
    stitched.extend_from_slice(&suffix[1..]);
    Some(stitched)
}

fn next_repath_backoff(current: u32) -> u32 {
    current
        .max(REPATH_TICKS)
        .saturating_mul(2)
        .min(MAX_REPATH_BACKOFF_TICKS)
}

/// Coarse navigation occupancy. Doors are handled by [`door_step_gate`] because their
/// blocking shape is an edge slab, not a whole blocked cell.
fn navigation_solid_fn(world: &World) -> impl Fn(IVec3) -> bool + '_ {
    move |c: IVec3| {
        let block = world.physics_block(c.x, c.y, c.z);
        if block.render_shape() == crate::block::RenderShape::Door {
            false
        } else {
            block.blocks_movement()
        }
    }
}

fn door_step_gate(world: &World, params: PathParams) -> impl Fn(IVec3, IVec3) -> bool + '_ {
    move |from, to| door_step_allowed(world, params, from, to)
}

fn door_step_allowed(world: &World, params: PathParams, from: IVec3, to: IVec3) -> bool {
    let dx = (to.x - from.x) as f32;
    let dz = (to.z - from.z) as f32;
    if dx == 0.0 && dz == 0.0 {
        return true;
    }

    let half_width = params.half_width.max(0.0);
    let head = params.head.max(1) as f32;
    let y = from.y.min(to.y) as f32;
    let cx = from.x as f32 + 0.5;
    let cz = from.z as f32 + 0.5;
    let min = [cx - half_width, y, cz - half_width];
    let max = [cx + half_width, y + head, cz + half_width];
    let (moved, _, _) = crate::collision::step_horizontal(min, max, dx, dz, 0.0, |x, y, z| {
        let block = world.physics_block(x, y, z);
        if block.render_shape() == crate::block::RenderShape::Door {
            world.collision_boxes_at(x, y, z)
        } else {
            &[]
        }
    });
    (moved[0] - dx).abs() < 1e-4 && (moved[2] - dz).abs() < 1e-4
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::Block;
    use crate::facing::Facing;

    #[test]
    fn idle_until_given_a_goal() {
        let nav = Navigator::new(1, 0.25);
        assert!(nav.is_idle());
    }

    #[test]
    fn arriving_consumes_waypoints_then_goes_idle() {
        let mut nav = Navigator::new(1, 0.25);
        // Hand-build a 2-step path so we don't need a World: start (0,1,0) -> (1,1,0).
        nav.path = vec![IVec3::new(0, 1, 0), IVec3::new(1, 1, 0)];
        nav.index = 1;
        nav.goal = Some(IVec3::new(1, 1, 0));
        nav.path_reaches_goal = true;
        // Standing on the waypoint: it's consumed and the nav goes idle.
        let on_wp = Vec3::new(1.5, 1.0, 0.5);
        let (wish, jump) = nav.follow(on_wp, true);
        assert_eq!(wish, Vec3::ZERO);
        assert!(!jump);
        assert!(nav.is_idle(), "arrived -> idle");
    }

    #[test]
    fn steers_toward_a_distant_waypoint() {
        let mut nav = Navigator::new(1, 0.25);
        nav.path = vec![IVec3::new(0, 1, 0), IVec3::new(5, 1, 0)];
        nav.index = 1;
        nav.goal = Some(IVec3::new(5, 1, 0));
        let (wish, jump) = nav.follow(Vec3::new(0.5, 1.0, 0.5), true);
        assert!(wish.x > 0.9, "heads +X toward the waypoint: {wish:?}");
        assert!(!jump, "flat move needs no jump");
    }

    #[test]
    fn jumps_when_close_to_a_step_up() {
        let mut nav = Navigator::new(1, 0.22);
        // Waypoint one block up and just ahead.
        nav.path = vec![IVec3::new(0, 1, 0), IVec3::new(1, 2, 0)];
        nav.index = 1;
        nav.goal = Some(IVec3::new(1, 2, 0));
        let (_wish, jump) = nav.follow(Vec3::new(0.7, 1.0, 0.5), true);
        assert!(jump, "should jump for a nearby one-block step up");
        // But not while airborne.
        let (_w2, jump_air) = nav.follow(Vec3::new(0.7, 1.0, 0.5), false);
        assert!(!jump_air, "no jump while off the ground");
    }

    #[test]
    fn vertical_bobbing_does_not_count_as_navigation_progress() {
        let mut nav = Navigator::new(1, 0.25);
        nav.path = vec![IVec3::new(0, 1, 0), IVec3::new(5, 1, 0)];
        nav.index = 1;
        nav.goal = Some(IVec3::new(5, 1, 0));
        nav.last_pos = Vec3::new(0.5, 1.0, 0.5);

        for tick in 0..STUCK_TICKS {
            let y = 1.0 + if tick % 2 == 0 { 0.2 } else { -0.2 };
            let (wish, _jump) = nav.follow(Vec3::new(0.5, y, 0.5), false);
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
        let mut nav = Navigator::new(1, 0.45);
        // Waypoint one block up in the adjacent cell. A wide mob standing in the lower
        // cell is already close to the ledge with its front edge even though its centre
        // is still a full block from the target centre.
        nav.path = vec![IVec3::new(0, 1, 0), IVec3::new(1, 2, 0)];
        nav.index = 1;
        nav.goal = Some(IVec3::new(1, 2, 0));
        let (_wish, jump) = nav.follow(Vec3::new(0.5, 1.0, 0.5), true);
        assert!(
            jump,
            "wider bodies jump before colliding with the step face"
        );
    }

    #[test]
    fn wide_mob_does_not_turn_before_clearing_a_corner() {
        let mut nav = Navigator::new(1, 0.45);
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
        let (wish, jump) = nav.follow(Vec3::new(1.25, 1.0, 0.5), true);
        assert!(
            wish.x > 0.9 && wish.z.abs() < 0.1,
            "wide mob should keep clearing the corner before turning: {wish:?}"
        );
        assert!(!jump);
    }

    /// A single chunk with a solid grass floor at `y = 63`, so footholds sit at
    /// `y = 64` across it — enough terrain for `find_path` to route over.
    fn flat_world() -> World {
        use crate::chunk::{Chunk, ChunkPos};
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
        let mut nav = Navigator::new(1, 0.25);

        nav.update_goal_when_supported(Some(goal), start, &world, true);

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
        let mut nav = Navigator::new(1, 0.25);

        nav.update_goal_when_supported(Some(goal), start, &world, true);

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
        let mut nav = Navigator::new(1, 0.25);

        nav.update_goal_when_supported(Some(goal), start, &world, true);

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
    fn re_paths_a_held_goal_when_the_world_changes() {
        // Hold one goal while the world changes underneath: the navigator must keep the
        // first route until REPATH_TICKS elapse, then refresh it to route around new
        // terrain — the whole point of periodic re-pathing.
        let mut world = flat_world();
        let start = IVec3::new(1, 64, 1);
        let goal = IVec3::new(8, 64, 1);
        let mut nav = Navigator::new(1, 0.25);

        // Initial path: a straight run along z = 1, passing through (4, 64, 1).
        nav.update_goal_when_supported(Some(goal), start, &world, true);
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
            nav.update_goal_when_supported(Some(goal), start, &world, true);
            assert_eq!(
                nav.path(),
                stale.as_slice(),
                "no re-path before the interval elapses"
            );
        }

        // The REPATH_TICKS-th held tick refreshes the route, which now avoids the wall.
        nav.update_goal_when_supported(Some(goal), start, &world, true);
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
            |c| world.water_cell_at(c.x, c.y, c.z),
        );
        assert_ne!(
            direct.get(1),
            Some(&old_waypoint),
            "fixture must have a direct refresh route that would pick a different first step"
        );

        let mut nav = Navigator::new(1, 0.25);
        nav.path = vec![start, old_waypoint, IVec3::new(2, 64, 2), goal];
        nav.index = 1;
        nav.goal = Some(goal);
        nav.path_reaches_goal = true;
        nav.since_path = REPATH_TICKS - 1;

        nav.update_goal_when_supported(Some(goal), start, &world, true);
        assert_eq!(
            nav.path().get(1),
            Some(&old_waypoint),
            "same-goal refresh keeps steering toward the current valid waypoint"
        );
    }

    #[test]
    fn unreachable_goal_backs_off_consecutive_same_goal_repaths() {
        let (world, start, goal) = pillar_world();
        let mut nav = Navigator::new(1, 0.25);

        nav.update_goal_when_supported(Some(goal), start, &world, true);
        assert_eq!(nav.recomputes(), 1);
        assert_ne!(
            nav.path().last(),
            Some(&goal),
            "the pillar top is unreachable, so the route is partial"
        );

        for _ in 0..REPATH_TICKS - 1 {
            nav.update_goal_when_supported(Some(goal), start, &world, true);
        }
        assert_eq!(nav.recomputes(), 1, "no retry before the base interval");

        nav.update_goal_when_supported(Some(goal), start, &world, true);
        assert_eq!(
            nav.recomputes(),
            2,
            "first same-goal retry happens at the base interval"
        );

        for _ in 0..(REPATH_TICKS * 2 - 1) {
            nav.update_goal_when_supported(Some(goal), start, &world, true);
        }
        assert_eq!(
            nav.recomputes(),
            2,
            "a second failed retry waits for the doubled backoff interval"
        );

        nav.update_goal_when_supported(Some(goal), start, &world, true);
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
        let mut nav = Navigator::new(1, 0.25);

        nav.update_goal_when_supported(Some(unreachable), start, &world, true);
        for _ in 0..REPATH_TICKS {
            nav.update_goal_when_supported(Some(unreachable), start, &world, true);
        }
        assert_eq!(
            nav.recomputes(),
            2,
            "the unreachable goal has entered backoff"
        );

        nav.update_goal_when_supported(Some(reachable), start, &world, true);
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
        let mut nav = Navigator::new(1, 0.25);

        nav.update_goal_when_supported(Some(goal), start, &world, true);
        assert_eq!(nav.recomputes(), 1);
        assert_eq!(nav.path().last(), Some(&goal));

        for expected in 2..=3 {
            for _ in 0..REPATH_TICKS - 1 {
                nav.update_goal_when_supported(Some(goal), start, &world, true);
            }
            assert_eq!(
                nav.recomputes(),
                expected - 1,
                "no early reachable-goal repath"
            );
            nav.update_goal_when_supported(Some(goal), start, &world, true);
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
        let mut nav = Navigator::new(1, 0.25);
        nav.update_goal_when_supported(Some(goal), start, &world, true);
        let stale: Vec<IVec3> = nav.path().to_vec();
        let blocked = IVec3::new(4, 64, 1);
        assert!(stale.contains(&blocked));

        world.set_block_world(blocked.x, blocked.y, blocked.z, Block::Stone);
        world.set_block_world(blocked.x, blocked.y + 1, blocked.z, Block::Stone);

        for _ in 0..REPATH_TICKS - 1 {
            nav.update_goal_when_supported(Some(goal), start, &world, true);
        }
        assert_eq!(nav.path(), stale.as_slice());

        for _ in 0..5 {
            nav.update_goal_when_supported(Some(goal), start, &world, false);
            assert_eq!(nav.path(), stale.as_slice(), "mid-air repath is paused");
        }

        nav.update_goal_when_supported(Some(goal), start, &world, true);
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
        let mut nav = Navigator::new(1, 0.25);
        nav.update_goal_when_supported(Some(goal), start, &world, true);

        // Drive enough held ticks to cross several re-path intervals AND the stuck limit,
        // following from a fixed position each tick so no progress is ever made.
        let wedged = Vec3::new(1.5, 64.0, 1.5);
        let mut gave_up = false;
        for _ in 0..STUCK_TICKS + REPATH_TICKS {
            nav.update_goal_when_supported(Some(goal), start, &world, true);
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
}
