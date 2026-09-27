use serde::Deserialize;

use petramond_math::math::{IVec3, Vec3};

use super::super::brain::{AiBehavior, AiCtx, BehaviorOutput};
use super::super::path::is_navigation_foothold_with;
use super::los;

const GOAL_SCAN_CELLS: i32 = 3;
const LOST_SIGHT_GIVE_UP_TICKS: u16 = 200;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChaseParams {
    radius: f64,
    give_up_radius: f64,
    #[serde(default)]
    sneak_radius_penalty: f64,
}

pub struct ChasePlayerAi {
    radius: f32,
    give_up_radius: f32,
    sneak_radius_penalty: f32,
    chasing: bool,
    lost_sight_ticks: u16,
}

impl ChasePlayerAi {
    #[cfg(test)]
    pub fn new(radius: f32, give_up_radius: f32) -> Self {
        Self::with_sneak_penalty(radius, give_up_radius, 0.0)
    }

    pub fn with_sneak_penalty(radius: f32, give_up_radius: f32, sneak_radius_penalty: f32) -> Self {
        ChasePlayerAi {
            radius,
            give_up_radius: give_up_radius.max(radius),
            sneak_radius_penalty,
            chasing: false,
            lost_sight_ticks: 0,
        }
    }

    pub(super) fn from_params(params: &serde_json::Value) -> Result<Self, String> {
        let p: ChaseParams = serde_json::from_value(params.clone()).map_err(|e| e.to_string())?;
        if p.radius.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
            return Err("radius must be > 0".into());
        }
        if p.give_up_radius < p.radius {
            return Err("give_up_radius must be >= radius".into());
        }
        if !matches!(
            p.sneak_radius_penalty.partial_cmp(&0.0),
            Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
        ) {
            return Err("sneak_radius_penalty must be >= 0".into());
        }
        Ok(ChasePlayerAi::with_sneak_penalty(
            p.radius as f32,
            p.give_up_radius as f32,
            p.sneak_radius_penalty as f32,
        ))
    }

    fn engage_radius(&self, sneaking: bool) -> f32 {
        if sneaking {
            (self.radius - self.sneak_radius_penalty).max(0.0)
        } else {
            self.radius
        }
    }
}

impl AiBehavior for ChasePlayerAi {
    fn tick(&mut self, ctx: &mut AiCtx) -> BehaviorOutput {
        let d2 = (ctx.player_pos - ctx.pos).length_squared();
        if self.chasing {
            if d2 > self.give_up_radius * self.give_up_radius {
                self.chasing = false;
                self.lost_sight_ticks = 0;
            } else {
                let body = ctx.pos + Vec3::new(0.0, ctx.head_height * 0.5, 0.0);
                if los::line_clear(ctx.world, body, ctx.player_pos) {
                    self.lost_sight_ticks = 0;
                } else {
                    self.lost_sight_ticks = self.lost_sight_ticks.saturating_add(1);
                    if self.lost_sight_ticks > LOST_SIGHT_GIVE_UP_TICKS {
                        self.chasing = false;
                        self.lost_sight_ticks = 0;
                    }
                }
            }
        } else {
            let engage = self.engage_radius(ctx.player_sneaking);
            if d2 <= engage * engage {
                let body = ctx.pos + Vec3::new(0.0, ctx.head_height * 0.5, 0.0);
                if los::line_clear(ctx.world, body, ctx.player_pos) {
                    self.chasing = true;
                    self.lost_sight_ticks = 0;
                }
            }
        }
        if !self.chasing {
            return BehaviorOutput::default();
        }
        BehaviorOutput {
            goal: goal_cell_near(ctx, ctx.player_pos),
            target: Some(crate::mob::EntityRef::Player(ctx.player_id)),
            ..Default::default()
        }
    }
}

pub(super) fn goal_cell_near(
    ctx: &AiCtx,
    pos: petramond_math::world_pos::WorldPos,
) -> Option<IVec3> {
    let cursor = ctx.world.cursor();
    let solid = super::super::nav::nav_solid_fn(&cursor);
    let support = super::super::nav::nav_support_fn(&cursor, ctx.half_width);
    let fluid = super::super::nav::nav_fluid_fn(&cursor);
    let params = ctx.path_params();
    let x = pos.x.floor() as i32;
    let z = pos.z.floor() as i32;
    let y0 = pos.y.floor() as i32;
    for d in 0..=GOAL_SCAN_CELLS {
        for y in [y0 - d, y0 + d] {
            let c = IVec3::new(x, y, z);
            if is_navigation_foothold_with(c, params, &solid, &support, &fluid) {
                return Some(c);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mob::MobRng;
    use crate::world::ServerWorld;
    use petramond_math::world_pos::WorldPos;
    use petramond_world::block::Block;
    use petramond_world::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};

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

    fn ctx<'a>(
        world: &'a ServerWorld,
        rng: &'a mut MobRng,
        pos: WorldPos,
        player: WorldPos,
    ) -> AiCtx<'a> {
        let mut c = crate::mob::behavior::test_support::ctx_at(world, rng, pos);
        c.half_width = 0.22;
        c.player_pos = player;
        c
    }

    #[test]
    fn chases_a_player_in_radius_toward_a_standable_cell() {
        let world = flat_world();
        let mut rng = MobRng::new(1);
        let mut ai = ChasePlayerAi::new(10.0, 14.0);
        let mob = WorldPos::new(2.5, 64.0, 2.5);
        let player = WorldPos::new(7.5, 64.9, 2.5);
        let goal = ai
            .tick(&mut ctx(&world, &mut rng, mob, player))
            .goal
            .expect("in-radius player produces a goal");
        assert_eq!(
            goal,
            petramond_math::math::IVec3::new(7, 64, 2),
            "the goal is the player's foothold cell"
        );
    }

    #[test]
    fn out_of_radius_player_is_ignored() {
        let world = flat_world();
        let mut rng = MobRng::new(1);
        let mut ai = ChasePlayerAi::new(4.0, 6.0);
        let mob = WorldPos::new(2.5, 64.0, 2.5);
        let player = WorldPos::new(12.5, 64.9, 2.5);
        assert_eq!(ai.tick(&mut ctx(&world, &mut rng, mob, player)).goal, None);
    }

    #[test]
    fn hysteresis_keeps_the_chase_until_past_give_up_radius() {
        let world = flat_world();
        let mut rng = MobRng::new(1);
        let mut ai = ChasePlayerAi::new(4.0, 9.0);
        let mob = WorldPos::new(2.5, 64.0, 2.5);

        let near = WorldPos::new(5.5, 64.9, 2.5);
        assert!(ai
            .tick(&mut ctx(&world, &mut rng, mob, near))
            .goal
            .is_some());
        let band = WorldPos::new(9.5, 64.9, 2.5);
        assert!(
            ai.tick(&mut ctx(&world, &mut rng, mob, band))
                .goal
                .is_some(),
            "an engaged chase persists inside the give_up band"
        );
        let far = WorldPos::new(13.5, 64.9, 2.5);
        assert_eq!(ai.tick(&mut ctx(&world, &mut rng, mob, far)).goal, None);
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
        let mob = WorldPos::new(2.5, 64.0, 2.5);
        let airborne = WorldPos::new(7.5, 80.0, 2.5);
        assert_eq!(
            ai.tick(&mut ctx(&world, &mut rng, mob, airborne)).goal,
            None,
            "no standable cell near the player -> the chase emits nothing"
        );
    }

    #[test]
    fn blocked_sight_gates_engagement_and_long_occlusion_breaks_chase() {
        let mut world = flat_world();
        let mut rng = MobRng::new(1);
        let mut ai = ChasePlayerAi::new(10.0, 14.0);
        let mob = WorldPos::new(2.5, 64.0, 2.5);
        let player = WorldPos::new(7.5, 64.9, 2.5);

        for y in 64..=66 {
            assert!(world.set_block_world(5, y, 2, Block::OakLeaves));
        }
        assert_eq!(
            ai.tick(&mut ctx(&world, &mut rng, mob, player)).goal,
            None,
            "an in-radius player behind colliding blocks does not engage the chase"
        );

        for y in 64..=66 {
            assert!(world.set_block_world(5, y, 2, Block::Air));
        }
        assert!(ai
            .tick(&mut ctx(&world, &mut rng, mob, player))
            .goal
            .is_some());

        for y in 64..=66 {
            assert!(world.set_block_world(5, y, 2, Block::OakLeaves));
        }
        for tick in 1..=LOST_SIGHT_GIVE_UP_TICKS {
            assert!(
                ai.tick(&mut ctx(&world, &mut rng, mob, player))
                    .goal
                    .is_some(),
                "brief sight loss is tolerated; tick {tick} should still chase"
            );
        }
        assert_eq!(
            ai.tick(&mut ctx(&world, &mut rng, mob, player)).goal,
            None,
            "more than {LOST_SIGHT_GIVE_UP_TICKS} consecutive no-LOS ticks ends the chase"
        );
    }

    #[test]
    fn sneaking_shrinks_the_engage_radius_but_not_an_engaged_chase() {
        let world = flat_world();
        let mut rng = MobRng::new(1);
        let mut ai = ChasePlayerAi::with_sneak_penalty(10.0, 14.0, 5.0);
        let mob = WorldPos::new(2.5, 64.0, 2.5);
        let player = WorldPos::new(9.5, 64.9, 2.5);

        let mut c = ctx(&world, &mut rng, mob, player);
        c.player_sneaking = true;
        assert_eq!(
            ai.tick(&mut c).goal,
            None,
            "a sneaking player at 7 blocks stays undetected"
        );

        assert!(ai
            .tick(&mut ctx(&world, &mut rng, mob, player))
            .goal
            .is_some());

        let mut c = ctx(&world, &mut rng, mob, player);
        c.player_sneaking = true;
        assert!(
            ai.tick(&mut c).goal.is_some(),
            "sneaking shrinks detection, never an engaged chase"
        );

        let mut ai = ChasePlayerAi::with_sneak_penalty(10.0, 14.0, 5.0);
        let near = WorldPos::new(6.5, 64.9, 2.5);
        let mut c = ctx(&world, &mut rng, mob, near);
        c.player_sneaking = true;
        assert!(ai.tick(&mut c).goal.is_some());
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
        assert!(
            ChasePlayerAi::from_params(&serde_json::json!({
                "radius": 8.0, "give_up_radius": 12.0, "sneak_radius_penalty": 5.0
            }))
            .is_ok(),
            "the optional sneak penalty is accepted"
        );
        assert!(
            ChasePlayerAi::from_params(&serde_json::json!({
                "radius": 8.0, "give_up_radius": 12.0, "sneak_radius_penalty": -1.0
            }))
            .is_err(),
            "a negative sneak penalty is refused"
        );
    }
}
