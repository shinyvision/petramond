use petramond_math::math::{IVec3, Vec3};
use petramond_math::world_pos::WorldPos;
use petramond_world::biome::Biome;
use petramond_world::block::Block;

use super::super::brain::{AiBehavior, AiCtx, BehaviorOutput};
use super::super::confined::ConfinedRegion;
use super::super::path::{body_or_floor_touches, is_navigation_foothold_with, PathParams};
use super::super::{Habitat, WanderCohesion, WanderTuning};

const PICK_ATTEMPTS: u32 = 24;

const REACH_ATTEMPTS: u32 = 5;

const MIN_REGION_WANDER_CELLS: usize = 4;

const MAX_BACKOFF_STEPS: u8 = 2;
const MIN_BACKOFF_RADIUS: i32 = 3;

fn backoff_radius(radius: i32, steps: u8) -> i32 {
    (radius >> steps.min(MAX_BACKOFF_STEPS) as i32).max(MIN_BACKOFF_RADIUS.min(radius))
}

const AVOID_ESCAPE: u32 = 5;

const FLUID_ESCAPE: u32 = 3;

const GROUND_ESCAPE: u32 = 5;

pub struct WanderAi {
    tuning: WanderTuning,
    habitat: &'static Habitat,
    avoid_fluids: bool,
    current: Option<IVec3>,
    exhausted_picks: u8,
}

impl WanderAi {
    pub fn new(tuning: WanderTuning, habitat: &'static Habitat, avoid_fluids: bool) -> Self {
        WanderAi {
            tuning,
            habitat,
            avoid_fluids,
            current: None,
            exhausted_picks: 0,
        }
    }
}

impl AiBehavior for WanderAi {
    fn tick(&mut self, ctx: &mut AiCtx) -> BehaviorOutput {
        let goal = if self.current.is_some() && !ctx.nav_idle {
            self.current
        } else {
            self.current = None;
            let escape_fluid = self.avoid_fluids && ctx.in_fluid.is_some();
            if escape_fluid || ctx.rng.next_f32() < self.tuning.chance_per_tick {
                let mut tuning = self.tuning;
                tuning.radius = backoff_radius(tuning.radius, self.exhausted_picks);
                let pick = pick_destination(ctx, tuning, self.habitat, self.avoid_fluids);
                self.current = pick.goal;
                if pick.goal.is_some() {
                    self.exhausted_picks = 0;
                } else if pick.exhausted {
                    self.exhausted_picks = self.exhausted_picks.saturating_add(1);
                }
            }
            self.current
        };
        BehaviorOutput {
            goal,
            ..Default::default()
        }
    }
}

struct Pick {
    goal: Option<IVec3>,
    exhausted: bool,
}

fn pick_destination(
    ctx: &mut AiCtx,
    tuning: WanderTuning,
    habitat: &Habitat,
    avoid_fluids: bool,
) -> Pick {
    if let Some(region) = ctx.confined_region {
        return Pick {
            goal: pick_region_destination(ctx, tuning, habitat, avoid_fluids, region),
            exhausted: false,
        };
    }
    let cursor = ctx.world.cursor();
    let solid = super::super::nav::nav_solid_fn(&cursor);
    let support = super::super::nav::nav_support_fn(&cursor, ctx.half_width);
    let fluid = super::super::nav::nav_fluid_fn(&cursor);
    let radius = tuning.radius;
    let r2 = radius * radius;
    let path_params = ctx.path_params();
    let cohesion = tuning.cohesion.map(|rule| {
        (
            rule,
            companion_within(ctx, rule, ctx.pos, rule.search_radius(radius)),
        )
    });
    let escape_fluid = avoid_fluids && ctx.in_fluid.is_some();
    let mut picker = Picker::new(AVOID_ESCAPE, FLUID_ESCAPE, GROUND_ESCAPE);
    let mut wet_fallback = None;
    let mut unreachable_seen = 0u32;
    for _ in 0..PICK_ATTEMPTS {
        let dx = ctx.rng.next_range(-radius, radius);
        let dz = ctx.rng.next_range(-radius, radius);
        if (dx == 0 && dz == 0) || dx * dx + dz * dz > r2 {
            continue;
        }
        let (x, z) = (ctx.cell.x + dx, ctx.cell.z + dz);
        let biome = match ctx.world.data().column_biome(x, z) {
            Some(id) => Biome::from_id(id),
            None => continue,
        };
        let fit = classify_biome(biome, habitat);
        if picker.reject_avoided(fit) {
            continue;
        }
        let Some(y) = nearest_navigation_foothold_y(
            x,
            z,
            ctx.cell.y,
            radius,
            path_params,
            &solid,
            &support,
            &fluid,
        ) else {
            continue;
        };
        let dest = IVec3::new(x, y, z);
        if body_occupied(ctx, dest) {
            continue;
        }
        let wet = body_or_floor_touches(dest, path_params, &fluid);
        if avoid_fluids && !escape_fluid && picker.reject_fluid(wet) {
            continue;
        }
        if picker.reject_ground(floor_avoided(ctx, tuning.avoid_ground, dest)) {
            continue;
        }
        if let Some((rule, origin_has_companion)) = cohesion {
            if reject_for_cohesion(ctx, rule, origin_has_companion, dest, radius) {
                continue;
            }
        }
        match super::super::nav::destination_reachable(
            ctx.world,
            ctx.cell,
            dest,
            path_params,
            ctx.head_height,
            ctx.reach,
        ) {
            None => break,
            Some(false) => {
                unreachable_seen += 1;
                if unreachable_seen >= REACH_ATTEMPTS {
                    break;
                }
                continue;
            }
            Some(true) => {}
        }
        if escape_fluid && wet {
            wet_fallback.get_or_insert(dest);
            continue;
        }
        if let Some(dest) = picker.offer(dest, fit) {
            return Pick {
                goal: Some(dest),
                exhausted: false,
            };
        }
    }
    let goal = picker.into_fallback().or(wet_fallback);
    Pick {
        exhausted: goal.is_none() && unreachable_seen >= REACH_ATTEMPTS,
        goal,
    }
}

fn pick_region_destination(
    ctx: &mut AiCtx,
    tuning: WanderTuning,
    habitat: &Habitat,
    avoid_fluids: bool,
    region: &ConfinedRegion,
) -> Option<IVec3> {
    if region.cells.len() < MIN_REGION_WANDER_CELLS {
        return None;
    }
    let cursor = ctx.world.cursor();
    let fluid = super::super::nav::nav_fluid_fn(&cursor);
    let radius = tuning.radius;
    let r2 = radius * radius;
    let path_params = ctx.path_params();
    let escape_fluid = avoid_fluids && ctx.in_fluid.is_some();
    let mut picker = Picker::new(AVOID_ESCAPE, FLUID_ESCAPE, GROUND_ESCAPE);
    let mut wet_fallback = None;
    for _ in 0..PICK_ATTEMPTS {
        let roll = ctx.rng.next_range(0, region.cells.len() as i32 - 1);
        let dest = region.cells[roll as usize];
        let (dx, dz) = (dest.x - ctx.cell.x, dest.z - ctx.cell.z);
        if (dx == 0 && dz == 0) || dx * dx + dz * dz > r2 {
            continue;
        }
        let fit = match ctx.world.data().column_biome(dest.x, dest.z) {
            Some(id) => classify_biome(Biome::from_id(id), habitat),
            None => continue,
        };
        if picker.reject_avoided(fit) {
            continue;
        }
        if body_occupied(ctx, dest) {
            continue;
        }
        let wet = body_or_floor_touches(dest, path_params, &fluid);
        if avoid_fluids {
            if escape_fluid && wet {
                wet_fallback.get_or_insert(dest);
                continue;
            }
            if picker.reject_fluid(wet) {
                continue;
            }
        }
        if picker.reject_ground(floor_avoided(ctx, tuning.avoid_ground, dest)) {
            continue;
        }
        if let Some(dest) = picker.offer(dest, fit) {
            return Some(dest);
        }
    }
    picker.into_fallback().or(wet_fallback)
}

fn floor_avoided(ctx: &AiCtx, avoid: &[Block], dest: IVec3) -> bool {
    !avoid.is_empty()
        && avoid.contains(&Block::from_id(ctx.world.data().chunk_block(
            dest.x,
            dest.y - 1,
            dest.z,
        )))
}

#[allow(clippy::too_many_arguments)]
fn nearest_navigation_foothold_y(
    x: i32,
    z: i32,
    y0: i32,
    radius: i32,
    params: PathParams,
    solid: &impl Fn(IVec3) -> bool,
    support: &impl Fn(IVec3) -> bool,
    fluid: &impl Fn(IVec3) -> bool,
) -> Option<i32> {
    for d in 0..=radius {
        for y in [y0 - d, y0 + d] {
            if is_navigation_foothold_with(IVec3::new(x, y, z), params, solid, support, fluid) {
                return Some(y);
            }
        }
    }
    None
}

fn companion_within_cell(ctx: &AiCtx, rule: WanderCohesion, cell: IVec3, radius: i32) -> bool {
    companion_within(
        ctx,
        rule,
        WorldPos::block_min(cell) + Vec3::new(0.5, 0.0, 0.5),
        radius,
    )
}

fn body_occupied(ctx: &AiCtx, dest: IVec3) -> bool {
    let center = WorldPos::block_min(dest) + Vec3::new(0.5, 0.0, 0.5);
    let hit = |pos: WorldPos, hw: f32, height: f32| {
        let d = pos - center;
        d.x.abs() < hw + ctx.half_width
            && d.z.abs() < hw + ctx.half_width
            && d.y < ctx.head_height
            && -d.y < height
    };
    let reach = ctx.half_width + ctx.mobs.max_half_extent();
    ctx.mobs.near(center, reach).any(|(i, m)| {
        if Some(i) == ctx.mob_index {
            return false;
        }
        let s = super::super::def(m.kind).size;
        hit(m.pos, s.half_width, s.height)
    }) || ctx.players.iter().any(|p| {
        let Some(body) = p.body else {
            return false;
        };
        let (mn, mx) = body.aabb();
        let hw = f64::from(ctx.half_width);
        mn[0] < center.x + hw
            && mx[0] > center.x - hw
            && mn[2] < center.z + hw
            && mx[2] > center.z - hw
            && mn[1] < center.y + f64::from(ctx.head_height)
            && mx[1] > center.y
    })
}

fn reject_for_cohesion(
    ctx: &AiCtx,
    rule: WanderCohesion,
    origin_has_companion: bool,
    dest: IVec3,
    radius: i32,
) -> bool {
    origin_has_companion && !companion_within_cell(ctx, rule, dest, radius)
}

fn companion_within(ctx: &AiCtx, rule: WanderCohesion, pos: WorldPos, radius: i32) -> bool {
    let r = radius.max(0) as f32;
    let r2 = r * r;
    ctx.mobs.near(pos, r).any(|(i, mob)| {
        if Some(i) == ctx.mob_index || mob.kind != rule.companion || mob.confined() {
            return false;
        }
        let d = mob.pos - pos;
        d.x * d.x + d.z * d.z <= r2
    })
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum BiomeFit {
    Preferred,
    Neutral,
    Avoided,
}

fn classify_biome(biome: Biome, habitat: &Habitat) -> BiomeFit {
    if habitat.prefer.contains(&biome) {
        BiomeFit::Preferred
    } else if habitat.avoid.contains(&biome) {
        BiomeFit::Avoided
    } else {
        BiomeFit::Neutral
    }
}

/// Take preferred biome right away. Keep the first allowed-but-unpreferred spot as a fallback.
/// Skip avoided biomes and, for fluid-averse mobs, fluid spots until they've been passed over
/// enough times; then the rule lifts, or a boxed-in mob would never move.
///
/// No world/RNG, so unit-tested directly. Caller samples the world and hands candidates in.
struct Picker {
    avoid_escape: u32,
    avoided_seen: u32,
    fluid_escape: u32,
    fluid_seen: u32,
    ground_escape: u32,
    ground_seen: u32,
    fallback: Option<IVec3>,
}

impl Picker {
    fn new(avoid_escape: u32, fluid_escape: u32, ground_escape: u32) -> Self {
        Picker {
            avoid_escape,
            avoided_seen: 0,
            fluid_escape,
            fluid_seen: 0,
            ground_escape,
            ground_seen: 0,
            fallback: None,
        }
    }

    fn reject_avoided(&mut self, fit: BiomeFit) -> bool {
        if fit == BiomeFit::Avoided && self.avoided_seen < self.avoid_escape {
            self.avoided_seen += 1;
            true
        } else {
            false
        }
    }

    fn reject_fluid(&mut self, in_fluid: bool) -> bool {
        if in_fluid && self.fluid_seen < self.fluid_escape {
            self.fluid_seen += 1;
            true
        } else {
            false
        }
    }

    fn reject_ground(&mut self, avoided: bool) -> bool {
        if avoided && self.ground_seen < self.ground_escape {
            self.ground_seen += 1;
            true
        } else {
            false
        }
    }

    fn offer(&mut self, dest: IVec3, fit: BiomeFit) -> Option<IVec3> {
        if fit == BiomeFit::Preferred {
            return Some(dest);
        }
        self.fallback.get_or_insert(dest);
        None
    }

    fn into_fallback(self) -> Option<IVec3> {
        self.fallback
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::mob::brain::AiMob;
    use crate::mob::spatial::MobSnapshot;
    use crate::mob::{Mob, MobRng, MobTagValue};
    use crate::world::ServerWorld;
    use petramond_world::block::Block;
    use petramond_world::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};

    fn habitat() -> Habitat {
        Habitat {
            avoid: &[Biome::PLAINS, Biome::DESERT],
            prefer: &[Biome::FOREST],
        }
    }

    fn make_ctx<'a>(
        world: &'a ServerWorld,
        rng: &'a mut MobRng,
        mobs: &'a MobSnapshot,
        mob_index: usize,
        pos: WorldPos,
    ) -> AiCtx<'a> {
        let mut c = crate::mob::behavior::test_support::ctx_at(world, rng, pos);
        c.head_height = 1.0;
        c.half_width = 0.45;
        c.head = 2;
        c.mob_index = Some(mob_index);
        c.mobs = mobs;
        c
    }

    fn flat_grass_world(extra: impl FnOnce(&mut Chunk)) -> ServerWorld {
        let mut world = ServerWorld::new(0, 1);
        let mut chunk = Chunk::new(0, 0);
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                chunk.set_block(x, 64, z, Block::Grass);
                chunk.set_biome(x, z, Biome::PLAINS.id());
            }
        }
        extra(&mut chunk);
        world.insert_chunk_for_test(ChunkPos::new(0, 0), chunk);
        world
    }

    static PLAINS_HABITAT: Habitat = Habitat {
        avoid: &[],
        prefer: &[Biome::PLAINS],
    };

    fn plains_habitat() -> &'static Habitat {
        &PLAINS_HABITAT
    }

    #[test]
    fn classify_sorts_biomes_into_prefer_avoid_neutral() {
        let h = habitat();
        assert_eq!(classify_biome(Biome::FOREST, &h), BiomeFit::Preferred);
        assert_eq!(classify_biome(Biome::PLAINS, &h), BiomeFit::Avoided);
        assert_eq!(classify_biome(Biome::DESERT, &h), BiomeFit::Avoided);
        assert_eq!(classify_biome(Biome::TAIGA, &h), BiomeFit::Neutral);
    }

    #[test]
    fn classify_prefers_over_avoids_when_a_biome_is_in_both() {
        let h = Habitat {
            avoid: &[Biome::FOREST],
            prefer: &[Biome::FOREST],
        };
        assert_eq!(classify_biome(Biome::FOREST, &h), BiomeFit::Preferred);
    }

    #[test]
    fn picker_takes_a_preferred_candidate_immediately() {
        let mut p = Picker::new(AVOID_ESCAPE, FLUID_ESCAPE, GROUND_ESCAPE);
        let neutral = IVec3::new(1, 0, 0);
        let preferred = IVec3::new(2, 0, 0);
        assert_eq!(p.offer(neutral, BiomeFit::Neutral), None);
        assert_eq!(p.offer(preferred, BiomeFit::Preferred), Some(preferred));
        assert_eq!(p.into_fallback(), Some(neutral));
    }

    #[test]
    fn picker_falls_back_to_the_first_neutral_when_no_preferred() {
        let mut p = Picker::new(AVOID_ESCAPE, FLUID_ESCAPE, GROUND_ESCAPE);
        let first = IVec3::new(1, 0, 0);
        assert_eq!(p.offer(first, BiomeFit::Neutral), None);
        assert_eq!(p.offer(IVec3::new(2, 0, 0), BiomeFit::Neutral), None);
        assert_eq!(
            p.into_fallback(),
            Some(first),
            "first allowed candidate wins the fallback"
        );
    }

    #[test]
    fn picker_rejects_avoided_until_the_escape_hatch_lifts_it() {
        let mut p = Picker::new(3, FLUID_ESCAPE, GROUND_ESCAPE);
        for _ in 0..3 {
            assert!(p.reject_avoided(BiomeFit::Avoided));
        }
        assert!(!p.reject_avoided(BiomeFit::Avoided));
        let spot = IVec3::new(7, 0, 0);
        assert_eq!(p.offer(spot, BiomeFit::Avoided), None);
        assert_eq!(p.into_fallback(), Some(spot));
    }

    #[test]
    fn picker_never_rejects_neutral_or_preferred() {
        let mut p = Picker::new(AVOID_ESCAPE, FLUID_ESCAPE, GROUND_ESCAPE);
        assert!(!p.reject_avoided(BiomeFit::Neutral));
        assert!(!p.reject_avoided(BiomeFit::Preferred));
    }

    #[test]
    fn companion_search_requires_another_active_desired_mob() {
        let world = ServerWorld::new(0, 1);
        let mut rng = MobRng::new(1);
        let rule = WanderCohesion {
            companion: Mob::Sheep,
            search_radius_multiplier: 2,
        };
        let mobs = [
            AiMob {
                id: 0,
                kind: Mob::Sheep,
                pos: WorldPos::new(0.5, 64.0, 0.5),
                active: true,
                tags: Default::default(),
            },
            AiMob {
                id: 0,
                kind: Mob::Owl,
                pos: WorldPos::new(2.5, 64.0, 0.5),
                active: true,
                tags: Default::default(),
            },
            AiMob {
                id: 0,
                kind: Mob::Sheep,
                pos: WorldPos::new(3.5, 64.0, 0.5),
                active: false,
                tags: Default::default(),
            },
        ];
        let snap = MobSnapshot::from_mobs(mobs.clone());
        let ctx = make_ctx(&world, &mut rng, &snap, 0, mobs[0].pos);
        assert!(
            !companion_within(&ctx, rule, mobs[0].pos, 5),
            "self, wrong kind, and inactive mobs do not count"
        );

        let mobs = [
            mobs[0].clone(),
            AiMob {
                id: 0,
                kind: Mob::Sheep,
                pos: WorldPos::new(4.5, 64.0, 0.5),
                active: true,
                tags: Default::default(),
            },
        ];
        let mut rng = MobRng::new(1);
        let snap = MobSnapshot::from_mobs(mobs.clone());
        let ctx = make_ctx(&world, &mut rng, &snap, 0, mobs[0].pos);
        assert!(
            companion_within(&ctx, rule, mobs[0].pos, 5),
            "an active desired mob inside the wander radius counts"
        );
    }

    #[test]
    fn cohesion_rejects_only_when_the_mob_started_grouped() {
        let world = ServerWorld::new(0, 1);
        let mut rng = MobRng::new(1);
        let rule = WanderCohesion {
            companion: Mob::Sheep,
            search_radius_multiplier: 2,
        };
        let mobs = [
            AiMob {
                id: 0,
                kind: Mob::Sheep,
                pos: WorldPos::new(0.5, 64.0, 0.5),
                active: true,
                tags: Default::default(),
            },
            AiMob {
                id: 0,
                kind: Mob::Sheep,
                pos: WorldPos::new(2.5, 64.0, 0.5),
                active: true,
                tags: Default::default(),
            },
        ];
        let snap = MobSnapshot::from_mobs(mobs.clone());
        let ctx = make_ctx(&world, &mut rng, &snap, 0, mobs[0].pos);

        assert!(
            reject_for_cohesion(&ctx, rule, true, IVec3::new(20, 64, 0), 5),
            "a grouped mob rejects destinations away from companions"
        );
        assert!(
            !reject_for_cohesion(&ctx, rule, true, IVec3::new(2, 64, 0), 5),
            "a grouped mob accepts destinations near companions"
        );
        assert!(
            !reject_for_cohesion(&ctx, rule, false, IVec3::new(20, 64, 0), 5),
            "an already-lonely mob does not spend extra work enforcing cohesion"
        );
    }

    #[test]
    fn cohesion_can_notice_a_herd_out_to_the_search_radius() {
        let world = ServerWorld::new(0, 1);
        let mut rng = MobRng::new(1);
        let rule = WanderCohesion {
            companion: Mob::Sheep,
            search_radius_multiplier: 2,
        };
        let mobs = [
            AiMob {
                id: 0,
                kind: Mob::Sheep,
                pos: WorldPos::new(0.5, 64.0, 0.5),
                active: true,
                tags: Default::default(),
            },
            AiMob {
                id: 0,
                kind: Mob::Sheep,
                pos: WorldPos::new(15.5, 64.0, 0.5),
                active: true,
                tags: Default::default(),
            },
        ];
        let snap = MobSnapshot::from_mobs(mobs.clone());
        let ctx = make_ctx(&world, &mut rng, &snap, 0, mobs[0].pos);

        assert!(
            !companion_within(&ctx, rule, mobs[0].pos, 10),
            "the companion is outside one wander radius"
        );
        assert!(
            companion_within(&ctx, rule, mobs[0].pos, rule.search_radius(10)),
            "the companion is still close enough to recover as herd"
        );
        assert!(
            reject_for_cohesion(&ctx, rule, true, IVec3::new(-9, 64, 0), 10),
            "a recovery wander rejects moving farther from that companion"
        );
        assert!(
            !reject_for_cohesion(&ctx, rule, true, IVec3::new(7, 64, 0), 10),
            "a recovery wander accepts moving back within one wander radius"
        );
    }

    #[test]
    fn cohesion_ignores_confined_companions() {
        let world = ServerWorld::new(0, 1);
        let mut rng = MobRng::new(1);
        let rule = WanderCohesion {
            companion: Mob::Sheep,
            search_radius_multiplier: 2,
        };
        let mobs = [
            AiMob {
                id: 0,
                kind: Mob::Sheep,
                pos: WorldPos::new(0.5, 64.0, 0.5),
                active: true,
                tags: Default::default(),
            },
            AiMob {
                id: 0,
                kind: Mob::Sheep,
                pos: WorldPos::new(2.5, 64.0, 0.5),
                active: true,
                tags: std::sync::Arc::new(BTreeMap::from([(
                    crate::mob::tags::CONFINED.to_string(),
                    MobTagValue::Bool(true),
                )])),
            },
        ];
        let snap = MobSnapshot::from_mobs(mobs.clone());
        let ctx = make_ctx(&world, &mut rng, &snap, 0, mobs[0].pos);

        assert!(
            !companion_within(&ctx, rule, mobs[0].pos, 5),
            "a free sheep should not count a confined sheep as a herd companion"
        );
        assert!(
            !reject_for_cohesion(&ctx, rule, false, IVec3::new(20, 64, 0), 5),
            "without a free companion, cohesion does not constrain the destination"
        );
    }

    #[test]
    fn a_destination_covered_by_another_body_is_rejected() {
        let world = ServerWorld::new(0, 1);
        let mut rng = MobRng::new(1);
        let mobs = [AiMob {
            id: 0,
            kind: Mob::Sheep,
            pos: WorldPos::new(3.5, 64.0, 0.5),
            active: true,
            tags: Default::default(),
        }];
        let snap = MobSnapshot::from_mobs(mobs.clone());
        let ctx = make_ctx(&world, &mut rng, &snap, 1, WorldPos::new(0.5, 64.0, 0.5));
        assert!(
            body_occupied(&ctx, IVec3::new(3, 64, 0)),
            "the other sheep's cell is covered"
        );
        assert!(
            !body_occupied(&ctx, IVec3::new(6, 64, 0)),
            "a clear cell is not covered"
        );
    }

    fn region_for(world: &ServerWorld, start: IVec3) -> crate::mob::confined::ConfinedRegion {
        let params = PathParams::for_body(2, 0.45);
        let cursor = world.cursor();
        let solid = crate::mob::nav::nav_solid_fn(&cursor);
        let support = crate::mob::nav::nav_support_fn(&cursor, 0.45);
        let fluid = crate::mob::nav::nav_fluid_fn(&cursor);
        let step = crate::mob::nav::navigation_step_gate(&cursor, params, 1.4);
        let loaded = crate::mob::nav::nav_loaded_fn(&cursor);
        crate::mob::confined::confined_region(
            start, params, &solid, &support, &fluid, &step, &loaded,
        )
        .expect("test area should read as confined")
    }

    fn wander_tuning(radius: i32) -> WanderTuning {
        WanderTuning {
            chance_per_tick: 1.0,
            radius,
            avoid_ground: &[],
            cohesion: None,
        }
    }

    #[test]
    fn a_confined_mob_wanders_only_within_its_region() {
        let world = flat_grass_world(|chunk| {
            for i in 5..=11 {
                for (x, z) in [(5, i), (11, i), (i, 5), (i, 11)] {
                    chunk.set_block(x, 65, z, Block::OakFence);
                }
            }
        });
        let region = region_for(&world, IVec3::new(8, 65, 8));
        let mut picked = 0;
        for seed in 0..20 {
            let mut rng = MobRng::new(seed);
            let mut ctx = make_ctx(
                &world,
                &mut rng,
                MobSnapshot::empty(),
                0,
                WorldPos::new(8.5, 65.0, 8.5),
            );
            ctx.confined_region = Some(&region);
            let mut ai = WanderAi::new(wander_tuning(10), plains_habitat(), true);
            if let Some(goal) = ai.tick(&mut ctx).goal {
                picked += 1;
                assert!(region.contains(goal), "goal {goal:?} escaped the pen");
            }
        }
        assert!(picked > 0, "a penned mob must still wander");
    }

    #[test]
    fn a_region_smaller_than_two_by_two_never_wanders() {
        let world = flat_grass_world(|chunk| {
            for x in 4..=7 {
                for z in 4..=6 {
                    if x == 4 || x == 7 || z == 4 || z == 6 {
                        chunk.set_block(x, 65, z, Block::OakFence);
                    }
                }
            }
        });
        let region = region_for(&world, IVec3::new(5, 65, 5));
        assert!(region.cells.len() < MIN_REGION_WANDER_CELLS);
        let mut rng = MobRng::new(3);
        let mut ctx = make_ctx(
            &world,
            &mut rng,
            MobSnapshot::empty(),
            0,
            WorldPos::new(5.5, 65.0, 5.5),
        );
        ctx.confined_region = Some(&region);
        let mut ai = WanderAi::new(wander_tuning(10), plains_habitat(), true);
        for _ in 0..50 {
            assert!(
                ai.tick(&mut ctx).goal.is_none(),
                "a boxed-in mob must not jitter between its two cells"
            );
        }
    }

    #[test]
    fn a_free_mob_never_picks_an_unreachable_destination() {
        let world = flat_grass_world(|chunk| {
            for i in 5..=11 {
                for (x, z) in [(5, i), (11, i), (i, 5), (i, 11)] {
                    for y in 65..68 {
                        chunk.set_block(x, y, z, Block::Stone);
                    }
                }
            }
        });
        let mut picked = 0;
        for seed in 0..30 {
            let mut rng = MobRng::new(seed);
            let mut ctx = make_ctx(
                &world,
                &mut rng,
                MobSnapshot::empty(),
                0,
                WorldPos::new(8.5, 65.0, 8.5),
            );
            let mut ai = WanderAi::new(wander_tuning(10), plains_habitat(), true);
            if let Some(goal) = ai.tick(&mut ctx).goal {
                picked += 1;
                assert!(
                    (6..=10).contains(&goal.x) && (6..=10).contains(&goal.z) && goal.y == 65,
                    "goal {goal:?} lies beyond the sealed walls"
                );
            }
        }
        assert!(picked > 0, "in-pen destinations are reachable and pickable");
    }

    #[test]
    fn an_exhausted_pick_reports_itself_and_the_horizon_backs_off() {
        let world = flat_grass_world(|chunk| {
            for x in 7..=9 {
                for z in 7..=9 {
                    if (x, z) != (8, 8) {
                        for y in 65..68 {
                            chunk.set_block(x, y, z, Block::Stone);
                        }
                    }
                }
            }
        });
        let mut rng = MobRng::new(1);
        let mut ctx = make_ctx(
            &world,
            &mut rng,
            MobSnapshot::empty(),
            0,
            WorldPos::new(8.5, 65.0, 8.5),
        );
        let pick = pick_destination(&mut ctx, wander_tuning(10), plains_habitat(), true);
        assert!(pick.goal.is_none() && pick.exhausted, "sealed = exhausted");

        assert_eq!(backoff_radius(10, 0), 10);
        assert_eq!(backoff_radius(10, 1), 5);
        assert_eq!(backoff_radius(10, 2), 3, "halving floors at the minimum");
        assert_eq!(backoff_radius(10, 9), 3, "steps clamp at the maximum");
        assert_eq!(backoff_radius(2, 2), 2, "the floor never exceeds the base");
    }

    #[test]
    fn a_mob_sealed_into_one_cell_cancels_the_wander() {
        let world = flat_grass_world(|chunk| {
            for x in 7..=9 {
                for z in 7..=9 {
                    if (x, z) != (8, 8) {
                        for y in 65..68 {
                            chunk.set_block(x, y, z, Block::Stone);
                        }
                    }
                }
            }
        });
        for seed in 0..10 {
            let mut rng = MobRng::new(seed);
            let mut ctx = make_ctx(
                &world,
                &mut rng,
                MobSnapshot::empty(),
                0,
                WorldPos::new(8.5, 65.0, 8.5),
            );
            let mut ai = WanderAi::new(wander_tuning(10), plains_habitat(), true);
            assert_eq!(ai.tick(&mut ctx).goal, None, "seed {seed}");
        }
    }

    #[test]
    fn picker_rejects_fluid_until_the_escape_hatch_lifts_it() {
        let mut p = Picker::new(AVOID_ESCAPE, 3, GROUND_ESCAPE);
        for _ in 0..3 {
            assert!(p.reject_fluid(true));
        }
        assert!(!p.reject_fluid(true));
        let wet = IVec3::new(4, 0, 0);
        assert_eq!(p.offer(wet, BiomeFit::Neutral), None);
        assert_eq!(
            p.into_fallback(),
            Some(wet),
            "settles for a fluid after the escape hatch"
        );
    }

    #[test]
    fn picker_rejects_avoided_ground_until_the_escape_hatch_lifts_it() {
        let mut p = Picker::new(AVOID_ESCAPE, FLUID_ESCAPE, 3);
        for _ in 0..3 {
            assert!(p.reject_ground(true));
        }
        assert!(!p.reject_ground(true));
        let rocky = IVec3::new(5, 0, 0);
        assert_eq!(p.offer(rocky, BiomeFit::Neutral), None);
        assert_eq!(p.into_fallback(), Some(rocky));
        let mut p = Picker::new(AVOID_ESCAPE, FLUID_ESCAPE, GROUND_ESCAPE);
        assert!(!p.reject_ground(false));
    }

    #[test]
    fn wander_steers_off_avoided_floors_but_never_freezes_on_them() {
        static AVOID_STONE: &[Block] = &[Block::Stone];
        let tuning = |radius| WanderTuning {
            chance_per_tick: 1.0,
            radius,
            avoid_ground: AVOID_STONE,
            cohesion: None,
        };
        let world = flat_grass_world(|chunk| {
            for z in 0..CHUNK_SZ {
                for x in 8..CHUNK_SX {
                    chunk.set_block(x, 64, z, Block::Stone);
                }
            }
        });
        let mut picked = 0;
        for seed in 0..30 {
            let mut rng = MobRng::new(seed);
            let mut ctx = make_ctx(
                &world,
                &mut rng,
                MobSnapshot::empty(),
                0,
                WorldPos::new(7.5, 65.0, 7.5),
            );
            let pick = pick_destination(&mut ctx, tuning(6), plains_habitat(), true);
            if let Some(goal) = pick.goal {
                picked += 1;
                assert!(
                    goal.x < 8,
                    "goal {goal:?} stands on the avoided stone floor"
                );
            }
        }
        assert!(picked > 0, "grass destinations are still picked");

        let world = flat_grass_world(|chunk| {
            for z in 0..CHUNK_SZ {
                for x in 0..CHUNK_SX {
                    chunk.set_block(x, 64, z, Block::Stone);
                }
            }
        });
        let mut picked = 0;
        for seed in 0..30 {
            let mut rng = MobRng::new(seed);
            let mut ctx = make_ctx(
                &world,
                &mut rng,
                MobSnapshot::empty(),
                0,
                WorldPos::new(7.5, 65.0, 7.5),
            );
            if pick_destination(&mut ctx, tuning(6), plains_habitat(), true)
                .goal
                .is_some()
            {
                picked += 1;
            }
        }
        assert!(
            picked > 0,
            "a mob amid avoided ground must still wander (and escape a cave)"
        );
    }

    #[test]
    fn picker_never_rejects_a_dry_candidate() {
        let mut p = Picker::new(AVOID_ESCAPE, FLUID_ESCAPE, GROUND_ESCAPE);
        assert!(!p.reject_fluid(false));
    }

    #[test]
    fn fluid_averse_mob_in_fluid_picks_without_waiting_for_wander_roll() {
        let world = flat_grass_world(|chunk| {
            chunk.set_fluid(8, 65, 8, Block::Water, 0);
        });
        let mut rng = MobRng::new(1);
        let mut ctx = make_ctx(
            &world,
            &mut rng,
            MobSnapshot::empty(),
            0,
            WorldPos::new(8.5, 65.2, 8.5),
        );
        ctx.cell = IVec3::new(8, 66, 8);
        ctx.in_fluid = Some(Block::Water);
        let mut ai = WanderAi::new(
            WanderTuning {
                chance_per_tick: 0.0,
                radius: 4,
                avoid_ground: &[],
                cohesion: None,
            },
            plains_habitat(),
            true,
        );

        let goal = ai.tick(&mut ctx).goal.expect("fluid escape goal");
        let fluid = |c: IVec3| world.data().fluid_cell_at(c.x, c.y, c.z);
        assert!(
            !body_or_floor_touches(goal, ctx.path_params(), &fluid),
            "dry land is preferred when it is available: {goal:?}"
        );
    }

    #[test]
    fn fluid_escape_falls_back_to_swimming_when_no_dry_target_is_sampled() {
        let world = flat_grass_world(|chunk| {
            for z in 0..CHUNK_SZ {
                for x in 0..CHUNK_SX {
                    chunk.set_fluid(x, 65, z, Block::Water, 0);
                }
            }
        });
        let mut rng = MobRng::new(1);
        let mut ctx = make_ctx(
            &world,
            &mut rng,
            MobSnapshot::empty(),
            0,
            WorldPos::new(8.5, 65.2, 8.5),
        );
        ctx.cell = IVec3::new(8, 66, 8);
        ctx.in_fluid = Some(Block::Water);
        let mut ai = WanderAi::new(
            WanderTuning {
                chance_per_tick: 0.0,
                radius: 4,
                avoid_ground: &[],
                cohesion: None,
            },
            plains_habitat(),
            true,
        );

        let goal = ai.tick(&mut ctx).goal.expect("fluid-surface fallback");
        let fluid = |c: IVec3| world.data().fluid_cell_at(c.x, c.y, c.z);
        assert!(
            body_or_floor_touches(goal, ctx.path_params(), &fluid),
            "without dry land, the mob should still swim to another fluid surface: {goal:?}"
        );
    }
}
