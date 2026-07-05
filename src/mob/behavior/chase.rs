//! Chase: hunt the player by steering navigation at their cell.
//!
//! While the player is within `radius` of the mob, the node emits the player's
//! navigation cell as the goal every tick — the navigator re-paths a changed goal at
//! once and a held goal every `REPATH_TICKS`, so a moving player is tracked without
//! any extra machinery here. Hysteresis: once engaged the chase only breaks past
//! `give_up_radius` (≥ `radius`), so a player skirting the aggro edge doesn't flicker
//! the mob between hunting and roaming.
//!
//! The goal is a *valid mob foothold* near the player (the same navigation-foothold
//! test the pathfinder uses), scanned vertically around the player's feet. A player
//! with no standable cell nearby (flying, deep water) yields no goal, and the merge
//! falls through to lower-priority locomotion.

use serde::Deserialize;

use crate::mathh::IVec3;

use super::super::brain::{AiBehavior, AiCtx, BehaviorOutput};
use super::super::path::{is_navigation_foothold, PathParams};

/// How many cells above/below the player's feet cell to scan for a mob-standable
/// goal (covers a player on a ledge lip, in shallow water, or mid-jump).
const GOAL_SCAN_CELLS: i32 = 3;

/// `chase_player` params as written in a `mobs.json` brain row.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChaseParams {
    /// Engage when the player is within this many blocks.
    radius: f64,
    /// Once engaged, keep chasing until the player is beyond this (≥ `radius`).
    give_up_radius: f64,
}

pub struct ChasePlayerAi {
    radius: f32,
    give_up_radius: f32,
    chasing: bool,
}

impl ChasePlayerAi {
    pub fn new(radius: f32, give_up_radius: f32) -> Self {
        ChasePlayerAi {
            radius,
            give_up_radius: give_up_radius.max(radius),
            chasing: false,
        }
    }

    /// Build from a brain row's `params` — the `chase_player` node factory core.
    pub(super) fn from_params(params: &serde_json::Value) -> Result<Self, String> {
        let p: ChaseParams = serde_json::from_value(params.clone()).map_err(|e| e.to_string())?;
        // `partial_cmp` (not `<=`) so a NaN radius is rejected too.
        if p.radius.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
            return Err("radius must be > 0".into());
        }
        if p.give_up_radius < p.radius {
            return Err("give_up_radius must be >= radius".into());
        }
        Ok(ChasePlayerAi::new(p.radius as f32, p.give_up_radius as f32))
    }
}

impl AiBehavior for ChasePlayerAi {
    fn tick(&mut self, ctx: &mut AiCtx) -> BehaviorOutput {
        let d2 = (ctx.player_pos - ctx.pos).length_squared();
        if self.chasing {
            if d2 > self.give_up_radius * self.give_up_radius {
                self.chasing = false;
            }
        } else if d2 <= self.radius * self.radius {
            self.chasing = true;
        }
        if !self.chasing {
            return BehaviorOutput::default();
        }
        BehaviorOutput {
            goal: player_goal_cell(ctx),
            ..Default::default()
        }
    }
}

/// The navigation-foothold cell nearest the player that THIS mob can stand in, or
/// `None` when no standable cell sits within the vertical scan (player airborne /
/// over deep water). Reuses the pathfinder's foothold test so the emitted goal is
/// always a cell `find_path` accepts.
fn player_goal_cell(ctx: &AiCtx) -> Option<IVec3> {
    let solid = |c: IVec3| ctx.world.blocks_movement_at(c.x, c.y, c.z);
    let water = |c: IVec3| ctx.world.water_cell_at(c.x, c.y, c.z);
    let params = PathParams::for_body(ctx.head, ctx.half_width);
    let x = ctx.player_pos.x.floor() as i32;
    let z = ctx.player_pos.z.floor() as i32;
    // `player_pos` is the body centre (feet + ~0.9), so its floor is the feet cell.
    let y0 = ctx.player_pos.y.floor() as i32;
    for d in 0..=GOAL_SCAN_CELLS {
        for y in [y0 - d, y0 + d] {
            let c = IVec3::new(x, y, z);
            if is_navigation_foothold(c, params, &solid, &water) {
                return Some(c);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::Block;
    use crate::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};
    use crate::mathh::Vec3;
    use crate::mob::MobRng;
    use crate::world::World;

    fn flat_world() -> World {
        let mut world = World::new(0, 1);
        let mut chunk = Chunk::new(0, 0);
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                chunk.set_block(x, 63, z, Block::Grass);
            }
        }
        world.insert_chunk_for_test(ChunkPos::new(0, 0), chunk);
        world
    }

    fn ctx<'a>(world: &'a World, rng: &'a mut MobRng, pos: Vec3, player: Vec3) -> AiCtx<'a> {
        AiCtx {
            pos,
            cell: crate::mathh::voxel_at(pos),
            yaw: 0.0,
            head_height: 0.7,
            half_width: 0.22,
            world,
            player_pos: player,
            nav_idle: true,
            in_water: false,
            head: 1,
            idle_anims: &[],
            mob_index: 0,
            mobs: &[],
            rng,
        }
    }

    #[test]
    fn chases_a_player_in_radius_toward_a_standable_cell() {
        let world = flat_world();
        let mut rng = MobRng::new(1);
        let mut ai = ChasePlayerAi::new(10.0, 14.0);
        // Player 5 blocks away, standing on the floor (body centre at feet + 0.9).
        let mob = Vec3::new(2.5, 64.0, 2.5);
        let player = Vec3::new(7.5, 64.9, 2.5);
        let goal = ai
            .tick(&mut ctx(&world, &mut rng, mob, player))
            .goal
            .expect("in-radius player produces a goal");
        assert_eq!(
            goal,
            crate::mathh::IVec3::new(7, 64, 2),
            "the goal is the player's foothold cell"
        );
    }

    #[test]
    fn out_of_radius_player_is_ignored() {
        let world = flat_world();
        let mut rng = MobRng::new(1);
        let mut ai = ChasePlayerAi::new(4.0, 6.0);
        let mob = Vec3::new(2.5, 64.0, 2.5);
        let player = Vec3::new(12.5, 64.9, 2.5); // 10 blocks: outside radius 4
        assert_eq!(ai.tick(&mut ctx(&world, &mut rng, mob, player)).goal, None);
    }

    #[test]
    fn hysteresis_keeps_the_chase_until_past_give_up_radius() {
        let world = flat_world();
        let mut rng = MobRng::new(1);
        let mut ai = ChasePlayerAi::new(4.0, 9.0);
        let mob = Vec3::new(2.5, 64.0, 2.5);

        // Engage inside `radius`...
        let near = Vec3::new(5.5, 64.9, 2.5); // 3 blocks
        assert!(ai
            .tick(&mut ctx(&world, &mut rng, mob, near))
            .goal
            .is_some());
        // ...keep chasing in the hysteresis band (past radius, short of give_up)...
        let band = Vec3::new(9.5, 64.9, 2.5); // 7 blocks
        assert!(
            ai.tick(&mut ctx(&world, &mut rng, mob, band))
                .goal
                .is_some(),
            "an engaged chase persists inside the give_up band"
        );
        // ...and break it past `give_up_radius`.
        let far = Vec3::new(13.5, 64.9, 2.5); // 11 blocks
        assert_eq!(ai.tick(&mut ctx(&world, &mut rng, mob, far)).goal, None);
        // Back in the band WITHOUT re-entering `radius`: no re-engage (hysteresis).
        assert_eq!(
            ai.tick(&mut ctx(&world, &mut rng, mob, band)).goal,
            None,
            "a broken chase only re-engages inside the (smaller) aggro radius"
        );
    }

    #[test]
    fn an_unstandable_player_position_yields_no_goal() {
        let world = flat_world();
        let mut rng = MobRng::new(1);
        let mut ai = ChasePlayerAi::new(30.0, 40.0);
        let mob = Vec3::new(2.5, 64.0, 2.5);
        // Player floating far above the floor: no foothold within the scan.
        let airborne = Vec3::new(7.5, 80.0, 2.5);
        assert_eq!(
            ai.tick(&mut ctx(&world, &mut rng, mob, airborne)).goal,
            None,
            "no standable cell near the player -> the chase emits nothing"
        );
    }

    #[test]
    fn params_are_validated_at_load() {
        assert!(ChasePlayerAi::from_params(
            &serde_json::json!({"radius": 8.0, "give_up_radius": 12.0})
        )
        .is_ok());
        assert!(
            ChasePlayerAi::from_params(&serde_json::json!({"radius": 8.0})).is_err(),
            "missing give_up_radius is refused"
        );
        assert!(
            ChasePlayerAi::from_params(&serde_json::json!({"radius": 8.0, "give_up_radius": 4.0}))
                .is_err(),
            "give_up_radius below radius is refused"
        );
        assert!(
            ChasePlayerAi::from_params(
                &serde_json::json!({"radius": 8.0, "give_up_radius": 12.0, "bogus": 1})
            )
            .is_err(),
            "unknown params are refused"
        );
    }
}
