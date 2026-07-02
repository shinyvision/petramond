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

pub struct Navigator {
    path: Vec<IVec3>,
    /// Index of the next waypoint to walk to.
    index: usize,
    goal: Option<IVec3>,
    params: PathParams,
    half_width: f32,
    stuck: u32,
    last_pos: Vec3,
    /// Ticks since the current path was computed; at [`REPATH_TICKS`] the held goal is
    /// re-pathed to refresh a route gone stale (see the constant).
    since_path: u32,
}

impl Navigator {
    pub fn new(head: i32, half_width: f32) -> Self {
        Navigator {
            path: Vec::new(),
            index: 0,
            goal: None,
            params: PathParams::for_body(head, half_width),
            half_width,
            stuck: 0,
            last_pos: Vec3::ZERO,
            since_path: 0,
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

    fn clear(&mut self) {
        self.path.clear();
        self.index = 0;
        self.goal = None;
        self.stuck = 0;
        self.since_path = 0;
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
                    // deliberate new destination, not the same one re-evaluated.
                    self.recompute(start, g, world);
                    self.goal = Some(g);
                    self.stuck = 0;
                } else {
                    // Same goal held: refresh the route every REPATH_TICKS. The stuck
                    // tally is left to keep climbing across refreshes, so a mob wedged the
                    // whole time still abandons the goal rather than re-pathing forever.
                    self.since_path += 1;
                    if self.since_path >= REPATH_TICKS {
                        self.recompute(start, g, world);
                    }
                }
            }
        }
    }

    /// (Re)compute the path from `start` to `goal`, resetting the waypoint cursor to the
    /// first step and the repath timer. Shared by a goal change and the periodic refresh.
    fn recompute(&mut self, start: IVec3, goal: IVec3, world: &World) {
        let solid = solid_fn(world);
        let water = |c: IVec3| world.water_cell_at(c.x, c.y, c.z);
        self.path = path::find_path(start, goal, self.params, solid, water);
        // Index 1 = the first cell to walk to (path[0] is the start).
        self.index = if self.path.len() > 1 {
            1
        } else {
            self.path.len()
        };
        self.since_path = 0;
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

        // Path exhausted — arrived. Going idle lets the brain choose what's next.
        self.clear();
        (Vec3::ZERO, false)
    }

    fn arrive_xz(&self) -> f32 {
        MAX_ARRIVE_XZ.min((0.5 - self.half_width).max(MIN_ARRIVE_XZ))
    }
}

/// `true` if a cell blocks movement (has a collision box), via the world's blocks.
fn solid_fn(world: &World) -> impl Fn(IVec3) -> bool + '_ {
    move |c: IVec3| world.blocks_movement_at(c.x, c.y, c.z)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::Block;

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
