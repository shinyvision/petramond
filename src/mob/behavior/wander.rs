//! Wander: the idle-roaming behavior.
//!
//! Each game tick, while the mob has no active path, there's a small chance it picks
//! a new random destination — a standable navigation foothold within a radius — and
//! hands it to the navigator. While the navigator is still walking the mob there,
//! wander simply keeps requesting that same destination (so the brain doesn't repath
//! every tick). When the mob arrives (or the navigator gives up), wander goes quiet
//! until its next random roll. Water-averse mobs that are already in water skip the
//! random roll and immediately look for a dry exit, falling back to water-surface
//! wandering if no dry destination is sampled.
//!
//! Destinations are filtered by the species' [`Habitat`]: avoided biomes are never
//! targeted (bar a bounded escape hatch so a hemmed-in mob still moves), and among
//! the rest preferred biomes win out — so, e.g., an owl hugs forest and drifts back
//! toward it after straying.
//!
//! Every pick must be REACHABLE (2026-07-20): a free mob's sampled spot is
//! verified with a bounded pathfinding probe and re-rolled when the route
//! doesn't actually arrive — up to [`REACH_ATTEMPTS`] failures, after which
//! the pick is cancelled. A CONFINED mob (see `mob::confined`) skips sampling
//! entirely and draws from its cached region of reachable cells; a region
//! smaller than 2×2 never wanders. Both rules exist for the same reason: the
//! pathfinder answers an unreachable goal with a best-effort partial route,
//! which walks the mob to the nearest wall cell and parks it there — penned
//! sheep spent their lives pressed against the fence chasing pasture they
//! could never reach.

use crate::biome::Biome;
use crate::mathh::{IVec3, Vec3};

use super::super::brain::{AiBehavior, AiCtx, BehaviorOutput};
use super::super::confined::ConfinedRegion;
use super::super::path::{body_or_floor_touches, is_navigation_foothold_with, PathParams};
use super::super::{Habitat, WanderCohesion, WanderTuning};

/// How many random offsets to try when picking a destination before giving up for
/// this tick (keeps the search cheap; it only runs on the occasional roll).
const PICK_ATTEMPTS: u32 = 24;

/// Reachability probes allowed per pick: a sampled destination the pathfinder
/// cannot actually reach (beyond a fence, over a wall) re-rolls; after this
/// many failed probes the pick is cancelled — the mob keeps whatever reachable
/// fallback it already saw, or stays put until the next wander roll. Without
/// this gate an unreachable pick walks the mob to the nearest wall cell and
/// parks it there (the sheep-hugging-the-fence bug).
const REACH_ATTEMPTS: u32 = 5;

/// A confined region smaller than 2×2 (fewer reachable cells than this) never
/// wanders: there is nowhere meaningful to go, and endlessly re-pathing inside
/// a one-block box is just jitter.
const MIN_REGION_WANDER_CELLS: usize = 4;

/// How many consecutive probe-exhausted picks shrink the wander horizon (each
/// step halves the radius, floored at [`MIN_BACKOFF_RADIUS`]). A pick that
/// cancels on unreachable probes is EVIDENCE the far part of the disc is
/// walled off — a mob near the wall of an enclosure too large to read as
/// confined, a cliff base, a shore — so instead of going quiet (the lethargy
/// failure mode of a flat retry cap), the next roll looks CLOSER, where
/// samples are likelier reachable: the free-mob analogue of a confined mob's
/// region-picking. Any successful pick resets the horizon.
const MAX_BACKOFF_STEPS: u8 = 2;
const MIN_BACKOFF_RADIUS: i32 = 3;

/// The wander radius after `steps` consecutive exhausted picks.
fn backoff_radius(radius: i32, steps: u8) -> i32 {
    (radius >> steps.min(MAX_BACKOFF_STEPS) as i32).max(MIN_BACKOFF_RADIUS.min(radius))
}

/// After this many avoided-biome candidates have been passed over in one pick, the
/// avoid rule lifts for the rest of that pick — so a mob boxed in by avoided terrain
/// isn't frozen, it just settles for the best it can reach.
const AVOID_ESCAPE: u32 = 5;

/// Same idea for water (for a water-averse species): re-roll a water destination this
/// many times, then accept a wet one rather than refuse to move. Crossing water on
/// the way to a dry destination is unaffected — that's the pathfinder's call.
const WATER_ESCAPE: u32 = 3;

pub struct WanderAi {
    tuning: WanderTuning,
    /// The species' biome affinity, consulted when choosing a destination.
    habitat: &'static Habitat,
    /// Whether to steer destinations away from water (with the bounded re-roll above).
    avoid_water: bool,
    /// The destination currently being walked to (if any).
    current: Option<IVec3>,
    /// Consecutive picks that exhausted their reachability probes — drives
    /// the horizon back-off (see [`backoff_radius`]). Reset by any success.
    exhausted_picks: u8,
}

impl WanderAi {
    pub fn new(tuning: WanderTuning, habitat: &'static Habitat, avoid_water: bool) -> Self {
        WanderAi {
            tuning,
            habitat,
            avoid_water,
            current: None,
            exhausted_picks: 0,
        }
    }
}

impl AiBehavior for WanderAi {
    fn tick(&mut self, ctx: &mut AiCtx) -> BehaviorOutput {
        // Still walking to the current destination: keep requesting it (no repath).
        let goal = if self.current.is_some() && !ctx.nav_idle {
            self.current
        } else {
            // Idle (arrived / gave up / never had one): drop the old target, and on
            // the occasional roll pick a fresh standable destination.
            self.current = None;
            let escape_water = self.avoid_water && ctx.in_water;
            if escape_water || ctx.rng.next_f32() < self.tuning.chance_per_tick {
                let mut tuning = self.tuning;
                tuning.radius = backoff_radius(tuning.radius, self.exhausted_picks);
                let pick = pick_destination(ctx, tuning, self.habitat, self.avoid_water);
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

/// The outcome of one destination pick: the goal (if any), and whether the
/// pick died by exhausting its reachability probes — the hemmed-in signal
/// that shrinks the next pick's horizon.
struct Pick {
    goal: Option<IVec3>,
    exhausted: bool,
}

/// Pick a random standable destination within `radius` of the mob, honoring the
/// `habitat` (see [`Picker`]), or `None` if nothing suitable turned up in a few
/// tries. Samples a horizontal offset (inside the radius), classifies that column's
/// biome, and — for columns that clear the avoid filter — finds the foothold in the
/// column nearest the mob's level.
fn pick_destination(
    ctx: &mut AiCtx,
    tuning: WanderTuning,
    habitat: &Habitat,
    avoid_water: bool,
) -> Pick {
    // A confined mob's world IS its region: pick from the cells it can
    // actually reach instead of sampling (and pathing toward) open ground
    // beyond the walls. Region picks never probe, so they never exhaust.
    if let Some(region) = ctx.confined_region {
        return Pick {
            goal: pick_region_destination(ctx, tuning, habitat, avoid_water, region),
            exhausted: false,
        };
    }
    let solid = super::super::nav::nav_solid_fn(ctx.world);
    let support = super::super::nav::nav_support_fn(ctx.world, ctx.half_width);
    let water = |c: IVec3| ctx.world.water_cell_at(c.x, c.y, c.z);
    let radius = tuning.radius;
    let r2 = radius * radius;
    let path_params = PathParams::for_body(ctx.head, ctx.half_width);
    let cohesion = tuning.cohesion.map(|rule| {
        (
            rule,
            companion_within(ctx, rule, ctx.pos, rule.search_radius(radius)),
        )
    });
    let escape_water = avoid_water && ctx.in_water;
    let mut picker = Picker::new(AVOID_ESCAPE, WATER_ESCAPE);
    let mut wet_fallback = None;
    let mut unreachable_seen = 0u32;
    for _ in 0..PICK_ATTEMPTS {
        let dx = ctx.rng.next_range(-radius, radius);
        let dz = ctx.rng.next_range(-radius, radius);
        if (dx == 0 && dz == 0) || dx * dx + dz * dz > r2 {
            continue;
        }
        let (x, z) = (ctx.cell.x + dx, ctx.cell.z + dz);
        // Unloaded columns can't be judged (and have no real blocks to stand on).
        let biome = match ctx.world.column_biome(x, z) {
            Some(id) => Biome::from_id(id),
            None => continue,
        };
        let fit = classify_biome(biome, habitat);
        // The avoid rule is about the *biome* of the spot, so reject before the
        // (more expensive) foothold scan — and count it toward the escape hatch.
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
            &water,
        ) else {
            continue;
        };
        let dest = IVec3::new(x, y, z);
        // A body already standing there is not a destination: the pathfinder's
        // soft entity costs bend the ROUTE around a crowd, but only this veto
        // keeps the mob from picking a GOAL inside it.
        if body_occupied(ctx, dest) {
            continue;
        }
        let wet = body_or_floor_touches(dest, path_params, &water);
        // For a water-averse species (not currently escaping water), re-roll a
        // destination that sits in water — up to the escape hatch, after which
        // a wet spot is accepted rather than refusing.
        if avoid_water && !escape_water && picker.reject_water(wet) {
            continue;
        }
        if let Some((rule, origin_has_companion)) = cohesion {
            if reject_for_cohesion(ctx, rule, origin_has_companion, dest, radius) {
                continue;
            }
        }
        // The expensive gate comes LAST: the spot must be genuinely reachable.
        // Fences and walls read solid to the pathfinder, and it answers an
        // unreachable goal with a best-effort partial route — which would walk
        // the mob to the nearest wall cell and park it there. A bounded number
        // of failed probes cancels the pick: the mob is likely hemmed in, and
        // more probes would just be a slow way to stand still.
        if !super::super::nav::destination_reachable(
            ctx.world,
            ctx.cell,
            dest,
            path_params,
            ctx.head_height,
        ) {
            unreachable_seen += 1;
            if unreachable_seen >= REACH_ATTEMPTS {
                break;
            }
            continue;
        }
        if escape_water && wet {
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
    // No preferred foothold turned up: fall back to the first allowed one we saw (a
    // neutral biome, or — once an escape hatch tripped — an avoided / wet one). If
    // the mob is actively escaping water and sampled no dry target, use the first
    // wet surface so it still swims instead of idling in place.
    let goal = picker.into_fallback().or(wet_fallback);
    Pick {
        exhausted: goal.is_none() && unreachable_seen >= REACH_ATTEMPTS,
        goal,
    }
}

/// Pick a wander destination for a CONFINED mob: sample straight from the
/// region's reachable cells — never beyond the walls, and no pathfinding
/// probes needed (membership IS reachability). The species' biome and water
/// preferences still apply through the shared [`Picker`]; herd cohesion does
/// not (the pen is the herd's whole world, and chasing a companion beyond the
/// fence would just re-create the fence-hugging this branch removes). A
/// region smaller than 2×2 never wanders at all.
fn pick_region_destination(
    ctx: &mut AiCtx,
    tuning: WanderTuning,
    habitat: &Habitat,
    avoid_water: bool,
    region: &ConfinedRegion,
) -> Option<IVec3> {
    if region.cells.len() < MIN_REGION_WANDER_CELLS {
        return None;
    }
    let water = |c: IVec3| ctx.world.water_cell_at(c.x, c.y, c.z);
    let radius = tuning.radius;
    let r2 = radius * radius;
    let path_params = PathParams::for_body(ctx.head, ctx.half_width);
    let escape_water = avoid_water && ctx.in_water;
    let mut picker = Picker::new(AVOID_ESCAPE, WATER_ESCAPE);
    let mut wet_fallback = None;
    for _ in 0..PICK_ATTEMPTS {
        let roll = ctx.rng.next_range(0, region.cells.len() as i32 - 1);
        let dest = region.cells[roll as usize];
        let (dx, dz) = (dest.x - ctx.cell.x, dest.z - ctx.cell.z);
        if (dx == 0 && dz == 0) || dx * dx + dz * dz > r2 {
            continue;
        }
        let fit = match ctx.world.column_biome(dest.x, dest.z) {
            Some(id) => classify_biome(Biome::from_id(id), habitat),
            None => continue,
        };
        if picker.reject_avoided(fit) {
            continue;
        }
        if body_occupied(ctx, dest) {
            continue;
        }
        let wet = body_or_floor_touches(dest, path_params, &water);
        if avoid_water {
            if escape_water && wet {
                wet_fallback.get_or_insert(dest);
                continue;
            }
            if picker.reject_water(wet) {
                continue;
            }
        }
        if let Some(dest) = picker.offer(dest, fit) {
            return Some(dest);
        }
    }
    picker.into_fallback().or(wet_fallback)
}

/// The navigation foothold Y in column `(x, z)` closest to `y0`, scanning outward
/// up to `radius` cells either way, or `None` if the column has no foothold in
/// range. Water surface cells count here, matching the pathfinder.
#[allow(clippy::too_many_arguments)]
fn nearest_navigation_foothold_y(
    x: i32,
    z: i32,
    y0: i32,
    radius: i32,
    params: PathParams,
    solid: &impl Fn(IVec3) -> bool,
    support: &impl Fn(IVec3) -> bool,
    water: &impl Fn(IVec3) -> bool,
) -> Option<i32> {
    for d in 0..=radius {
        for y in [y0 - d, y0 + d] {
            if is_navigation_foothold_with(IVec3::new(x, y, z), params, solid, support, water) {
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
        Vec3::new(cell.x as f32 + 0.5, cell.y as f32, cell.z as f32 + 0.5),
        radius,
    )
}

/// Whether another active entity's body already covers the arrival footprint
/// at `dest` — wandering there would just press into them. Read from the tick
/// snapshot (self excluded): a best-effort veto, not a reservation.
fn body_occupied(ctx: &AiCtx, dest: IVec3) -> bool {
    let center = Vec3::new(dest.x as f32 + 0.5, dest.y as f32, dest.z as f32 + 0.5);
    let hit = |pos: Vec3, hw: f32, height: f32| {
        (pos.x - center.x).abs() < hw + ctx.half_width
            && (pos.z - center.z).abs() < hw + ctx.half_width
            && pos.y < center.y + ctx.head_height
            && center.y < pos.y + height
    };
    ctx.mobs.iter().enumerate().any(|(i, m)| {
        if i == ctx.mob_index || !m.active {
            return false;
        }
        let s = super::super::def(m.kind).size;
        hit(m.pos, s.half_width, s.height)
    }) || ctx.players.iter().any(|p| {
        let Some(body) = p.body else {
            return false;
        };
        let (mn, mx) = body.aabb();
        let hw = ctx.half_width;
        mn.x < center.x + hw
            && mx.x > center.x - hw
            && mn.z < center.z + hw
            && mx.z > center.z - hw
            && mn.y < center.y + ctx.head_height
            && mx.y > center.y
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

fn companion_within(ctx: &AiCtx, rule: WanderCohesion, pos: Vec3, radius: i32) -> bool {
    let r = radius.max(0) as f32;
    let r2 = r * r;
    ctx.mobs.iter().enumerate().any(|(i, mob)| {
        if i == ctx.mob_index || !mob.active || mob.kind != rule.companion || mob.confined() {
            return false;
        }
        let dx = mob.pos.x - pos.x;
        let dz = mob.pos.z - pos.z;
        dx * dx + dz * dz <= r2
    })
}

/// How a candidate column's biome sits with the species' [`Habitat`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum BiomeFit {
    Preferred,
    Neutral,
    Avoided,
}

/// Classify `biome` against `habitat`. Preference wins over avoidance if a biome
/// somehow appears in both lists (they're meant to be disjoint).
fn classify_biome(biome: Biome, habitat: &Habitat) -> BiomeFit {
    if habitat.prefer.contains(&biome) {
        BiomeFit::Preferred
    } else if habitat.avoid.contains(&biome) {
        BiomeFit::Avoided
    } else {
        BiomeFit::Neutral
    }
}

/// Turns the stream of footholds wander samples into one chosen destination, encoding
/// the destination policy: a preferred-biome foothold is taken at once; the first
/// allowed-but-unpreferred one is held as a fallback; avoided-biome and (for a
/// water-averse mob) in-water spots are skipped until their respective escape hatches
/// have been passed over enough times, after which the rule lifts (so a mob boxed in
/// by avoided terrain or water still gets to move).
///
/// Pure (no world / RNG), so the policy is unit-tested directly; the caller feeds it
/// the candidates it samples from the world.
struct Picker {
    avoid_escape: u32,
    avoided_seen: u32,
    water_escape: u32,
    water_seen: u32,
    fallback: Option<IVec3>,
}

impl Picker {
    fn new(avoid_escape: u32, water_escape: u32) -> Self {
        Picker {
            avoid_escape,
            avoided_seen: 0,
            water_escape,
            water_seen: 0,
            fallback: None,
        }
    }

    /// Should this candidate be skipped for sitting in an avoided biome? Counts the
    /// skip toward the escape hatch; once `avoid_escape` are counted the rule lifts
    /// and avoided biomes stop being rejected here (they become fallback-eligible).
    fn reject_avoided(&mut self, fit: BiomeFit) -> bool {
        if fit == BiomeFit::Avoided && self.avoided_seen < self.avoid_escape {
            self.avoided_seen += 1;
            true
        } else {
            false
        }
    }

    /// Should this candidate be skipped for being in water? Counts the skip toward the
    /// water escape hatch; once `water_escape` are counted the rule lifts and wet spots
    /// become fallback-eligible. Only consulted for water-averse species.
    fn reject_water(&mut self, in_water: bool) -> bool {
        if in_water && self.water_seen < self.water_escape {
            self.water_seen += 1;
            true
        } else {
            false
        }
    }

    /// Offer a standable candidate that already cleared the avoid filter. A preferred
    /// biome is returned to take at once; anything else is kept as the fallback (first
    /// one wins) and `None` keeps the search going for a preferred spot.
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
    use crate::block::Block;
    use crate::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};
    use crate::mob::brain::AiMob;
    use crate::mob::{Mob, MobRng, MobTagValue};
    use crate::world::World;

    fn habitat() -> Habitat {
        Habitat {
            avoid: &[Biome::Plains, Biome::Desert],
            prefer: &[Biome::Forest],
        }
    }

    fn make_ctx<'a>(
        world: &'a World,
        rng: &'a mut MobRng,
        mobs: &'a [AiMob],
        mob_index: usize,
        pos: Vec3,
    ) -> AiCtx<'a> {
        let mut c = crate::mob::behavior::test_support::ctx_at(world, rng, pos);
        c.head_height = 1.0;
        c.half_width = 0.45;
        c.head = 2;
        c.mob_index = mob_index;
        c.mobs = mobs;
        c
    }

    fn flat_grass_world(extra: impl FnOnce(&mut Chunk)) -> World {
        let mut world = World::new(0, 1);
        let mut chunk = Chunk::new(0, 0);
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                chunk.set_block(x, 64, z, Block::Grass);
                chunk.set_biome(x, z, Biome::Plains.id());
            }
        }
        extra(&mut chunk);
        world.insert_chunk_for_test(ChunkPos::new(0, 0), chunk);
        world
    }

    static PLAINS_HABITAT: Habitat = Habitat {
        avoid: &[],
        prefer: &[Biome::Plains],
    };

    fn plains_habitat() -> &'static Habitat {
        &PLAINS_HABITAT
    }

    #[test]
    fn classify_sorts_biomes_into_prefer_avoid_neutral() {
        let h = habitat();
        assert_eq!(classify_biome(Biome::Forest, &h), BiomeFit::Preferred);
        assert_eq!(classify_biome(Biome::Plains, &h), BiomeFit::Avoided);
        assert_eq!(classify_biome(Biome::Desert, &h), BiomeFit::Avoided);
        // A biome on neither list is fair game, just not favored.
        assert_eq!(classify_biome(Biome::Taiga, &h), BiomeFit::Neutral);
    }

    #[test]
    fn classify_prefers_over_avoids_when_a_biome_is_in_both() {
        let h = Habitat {
            avoid: &[Biome::Forest],
            prefer: &[Biome::Forest],
        };
        assert_eq!(classify_biome(Biome::Forest, &h), BiomeFit::Preferred);
    }

    #[test]
    fn picker_takes_a_preferred_candidate_immediately() {
        let mut p = Picker::new(AVOID_ESCAPE, WATER_ESCAPE);
        let neutral = IVec3::new(1, 0, 0);
        let preferred = IVec3::new(2, 0, 0);
        // A neutral spot is only remembered, not taken...
        assert_eq!(p.offer(neutral, BiomeFit::Neutral), None);
        // ...but a preferred one is taken on the spot, leaving the neutral as fallback.
        assert_eq!(p.offer(preferred, BiomeFit::Preferred), Some(preferred));
        assert_eq!(p.into_fallback(), Some(neutral));
    }

    #[test]
    fn picker_falls_back_to_the_first_neutral_when_no_preferred() {
        let mut p = Picker::new(AVOID_ESCAPE, WATER_ESCAPE);
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
        let mut p = Picker::new(3, WATER_ESCAPE);
        // The first 3 avoided candidates are rejected (counting toward the hatch)...
        for _ in 0..3 {
            assert!(p.reject_avoided(BiomeFit::Avoided));
        }
        // ...after which the rule lifts and avoided candidates stop being rejected.
        assert!(!p.reject_avoided(BiomeFit::Avoided));
        // Now an avoided spot is fallback-eligible (treated like a neutral one).
        let spot = IVec3::new(7, 0, 0);
        assert_eq!(p.offer(spot, BiomeFit::Avoided), None);
        assert_eq!(p.into_fallback(), Some(spot));
    }

    #[test]
    fn picker_never_rejects_neutral_or_preferred() {
        let mut p = Picker::new(AVOID_ESCAPE, WATER_ESCAPE);
        assert!(!p.reject_avoided(BiomeFit::Neutral));
        assert!(!p.reject_avoided(BiomeFit::Preferred));
    }

    #[test]
    fn companion_search_requires_another_active_desired_mob() {
        let world = World::new(0, 1);
        let mut rng = MobRng::new(1);
        let rule = WanderCohesion {
            companion: Mob::Sheep,
            search_radius_multiplier: 2,
        };
        let mobs = [
            AiMob {
                id: 0,
                kind: Mob::Sheep,
                pos: Vec3::new(0.5, 64.0, 0.5),
                active: true,
                tags: Default::default(),
            },
            AiMob {
                id: 0,
                kind: Mob::Owl,
                pos: Vec3::new(2.5, 64.0, 0.5),
                active: true,
                tags: Default::default(),
            },
            AiMob {
                id: 0,
                kind: Mob::Sheep,
                pos: Vec3::new(3.5, 64.0, 0.5),
                active: false,
                tags: Default::default(),
            },
        ];
        let ctx = make_ctx(&world, &mut rng, &mobs, 0, mobs[0].pos);
        assert!(
            !companion_within(&ctx, rule, mobs[0].pos, 5),
            "self, wrong kind, and inactive mobs do not count"
        );

        let mobs = [
            mobs[0].clone(),
            AiMob {
                id: 0,
                kind: Mob::Sheep,
                pos: Vec3::new(4.5, 64.0, 0.5),
                active: true,
                tags: Default::default(),
            },
        ];
        let mut rng = MobRng::new(1);
        let ctx = make_ctx(&world, &mut rng, &mobs, 0, mobs[0].pos);
        assert!(
            companion_within(&ctx, rule, mobs[0].pos, 5),
            "an active desired mob inside the wander radius counts"
        );
    }

    #[test]
    fn cohesion_rejects_only_when_the_mob_started_grouped() {
        let world = World::new(0, 1);
        let mut rng = MobRng::new(1);
        let rule = WanderCohesion {
            companion: Mob::Sheep,
            search_radius_multiplier: 2,
        };
        let mobs = [
            AiMob {
                id: 0,
                kind: Mob::Sheep,
                pos: Vec3::new(0.5, 64.0, 0.5),
                active: true,
                tags: Default::default(),
            },
            AiMob {
                id: 0,
                kind: Mob::Sheep,
                pos: Vec3::new(2.5, 64.0, 0.5),
                active: true,
                tags: Default::default(),
            },
        ];
        let ctx = make_ctx(&world, &mut rng, &mobs, 0, mobs[0].pos);

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
        let world = World::new(0, 1);
        let mut rng = MobRng::new(1);
        let rule = WanderCohesion {
            companion: Mob::Sheep,
            search_radius_multiplier: 2,
        };
        let mobs = [
            AiMob {
                id: 0,
                kind: Mob::Sheep,
                pos: Vec3::new(0.5, 64.0, 0.5),
                active: true,
                tags: Default::default(),
            },
            AiMob {
                id: 0,
                kind: Mob::Sheep,
                pos: Vec3::new(15.5, 64.0, 0.5),
                active: true,
                tags: Default::default(),
            },
        ];
        let ctx = make_ctx(&world, &mut rng, &mobs, 0, mobs[0].pos);

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
        let world = World::new(0, 1);
        let mut rng = MobRng::new(1);
        let rule = WanderCohesion {
            companion: Mob::Sheep,
            search_radius_multiplier: 2,
        };
        let mobs = [
            AiMob {
                id: 0,
                kind: Mob::Sheep,
                pos: Vec3::new(0.5, 64.0, 0.5),
                active: true,
                tags: Default::default(),
            },
            AiMob {
                id: 0,
                kind: Mob::Sheep,
                pos: Vec3::new(2.5, 64.0, 0.5),
                active: true,
                tags: std::sync::Arc::new(BTreeMap::from([(
                    crate::mob::tags::CONFINED.to_string(),
                    MobTagValue::Bool(true),
                )])),
            },
        ];
        let ctx = make_ctx(&world, &mut rng, &mobs, 0, mobs[0].pos);

        assert!(
            !companion_within(&ctx, rule, mobs[0].pos, 5),
            "a free sheep should not count a confined sheep as a herd companion"
        );
        // With no free companion seen at the origin, the mob is treated as already
        // lonely and cohesion does not constrain its destination.
        assert!(
            !reject_for_cohesion(&ctx, rule, false, IVec3::new(20, 64, 0), 5),
            "without a free companion, cohesion does not constrain the destination"
        );
    }

    #[test]
    fn a_destination_covered_by_another_body_is_rejected() {
        let world = World::new(0, 1);
        let mut rng = MobRng::new(1);
        let mobs = [AiMob {
            id: 0,
            kind: Mob::Sheep,
            pos: Vec3::new(3.5, 64.0, 0.5),
            active: true,
            tags: Default::default(),
        }];
        let ctx = make_ctx(&world, &mut rng, &mobs, 1, Vec3::new(0.5, 64.0, 0.5));
        assert!(
            body_occupied(&ctx, IVec3::new(3, 64, 0)),
            "the other sheep's cell is covered"
        );
        assert!(
            !body_occupied(&ctx, IVec3::new(6, 64, 0)),
            "a clear cell is not covered"
        );
    }

    /// The real confinement fill for the tests below, so region-driven picks
    /// are exercised against exactly what the instance refresh would cache.
    fn region_for(world: &World, start: IVec3) -> crate::mob::confined::ConfinedRegion {
        let params = PathParams::for_body(2, 0.45);
        let solid = crate::mob::nav::nav_solid_fn(world);
        let support = crate::mob::nav::nav_support_fn(world, 0.45);
        let water = |c: IVec3| world.water_cell_at(c.x, c.y, c.z);
        let step = crate::mob::nav::partial_step_gate(world, params, 1.4);
        let loaded = |c: IVec3| world.physics_cell_final_at(c.x, c.y, c.z);
        crate::mob::confined::confined_region(
            start, params, &solid, &support, &water, &step, &loaded,
        )
        .expect("test area should read as confined")
    }

    fn wander_tuning(radius: i32) -> WanderTuning {
        WanderTuning {
            chance_per_tick: 1.0,
            radius,
            cohesion: None,
        }
    }

    #[test]
    fn a_confined_mob_wanders_only_within_its_region() {
        // 5×5 fence pen: the wander radius (10) reaches far beyond it, but a
        // confined mob draws destinations from its region, never outside.
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
            let mut ctx = make_ctx(&world, &mut rng, &[], 0, Vec3::new(8.5, 65.0, 8.5));
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
        // 1×2 interior: room to exist, no room worth pacing.
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
        let mut ctx = make_ctx(&world, &mut rng, &[], 0, Vec3::new(5.5, 65.0, 5.5));
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
        // The mob is walled in but (with no cached region on the ctx —
        // detection hasn't run yet / just got invalidated) doesn't know it:
        // sampled spots beyond the stone walls must be rejected by the
        // reachability probe, so any goal that comes back lies inside.
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
            let mut ctx = make_ctx(&world, &mut rng, &[], 0, Vec3::new(8.5, 65.0, 8.5));
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
        // The hemmed-in signal must be distinguishable from "no candidates at
        // all", and the back-off must halve toward its floor and reset never
        // below the species' own radius.
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
        let mut ctx = make_ctx(&world, &mut rng, &[], 0, Vec3::new(8.5, 65.0, 8.5));
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
        // Nothing but the cell it stands on is reachable: five failed probes
        // cancel the pick instead of walking the mob into a wall forever.
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
            let mut ctx = make_ctx(&world, &mut rng, &[], 0, Vec3::new(8.5, 65.0, 8.5));
            let mut ai = WanderAi::new(wander_tuning(10), plains_habitat(), true);
            assert_eq!(ai.tick(&mut ctx).goal, None, "seed {seed}");
        }
    }

    #[test]
    fn picker_rejects_water_until_the_escape_hatch_lifts_it() {
        let mut p = Picker::new(AVOID_ESCAPE, 3);
        // The first 3 wet candidates are rejected (counting toward the hatch)...
        for _ in 0..3 {
            assert!(p.reject_water(true));
        }
        // ...after which water stops being rejected and a wet spot is fallback-eligible.
        assert!(!p.reject_water(true));
        let wet = IVec3::new(4, 0, 0);
        assert_eq!(p.offer(wet, BiomeFit::Neutral), None);
        assert_eq!(
            p.into_fallback(),
            Some(wet),
            "settles for water after the escape hatch"
        );
    }

    #[test]
    fn picker_never_rejects_a_dry_candidate() {
        let mut p = Picker::new(AVOID_ESCAPE, WATER_ESCAPE);
        assert!(!p.reject_water(false));
    }

    #[test]
    fn water_averse_mob_in_water_picks_without_waiting_for_wander_roll() {
        let world = flat_grass_world(|chunk| {
            chunk.set_water(8, 65, 8, Block::Water, 0);
        });
        let mut rng = MobRng::new(1);
        let mut ctx = make_ctx(&world, &mut rng, &[], 0, Vec3::new(8.5, 65.2, 8.5));
        ctx.cell = IVec3::new(8, 66, 8);
        ctx.in_water = true;
        let mut ai = WanderAi::new(
            WanderTuning {
                chance_per_tick: 0.0,
                radius: 4,
                cohesion: None,
            },
            plains_habitat(),
            true,
        );

        let goal = ai.tick(&mut ctx).goal.expect("water escape goal");
        let water = |c: IVec3| world.water_cell_at(c.x, c.y, c.z);
        assert!(
            !body_or_floor_touches(goal, PathParams::for_body(ctx.head, ctx.half_width), &water),
            "dry land is preferred when it is available: {goal:?}"
        );
    }

    #[test]
    fn water_escape_falls_back_to_swimming_when_no_dry_target_is_sampled() {
        let world = flat_grass_world(|chunk| {
            for z in 0..CHUNK_SZ {
                for x in 0..CHUNK_SX {
                    chunk.set_water(x, 65, z, Block::Water, 0);
                }
            }
        });
        let mut rng = MobRng::new(1);
        let mut ctx = make_ctx(&world, &mut rng, &[], 0, Vec3::new(8.5, 65.2, 8.5));
        ctx.cell = IVec3::new(8, 66, 8);
        ctx.in_water = true;
        let mut ai = WanderAi::new(
            WanderTuning {
                chance_per_tick: 0.0,
                radius: 4,
                cohesion: None,
            },
            plains_habitat(),
            true,
        );

        let goal = ai.tick(&mut ctx).goal.expect("water-surface fallback");
        let water = |c: IVec3| world.water_cell_at(c.x, c.y, c.z);
        assert!(
            body_or_floor_touches(goal, PathParams::for_body(ctx.head, ctx.half_width), &water),
            "without dry land, the mob should still swim to another water surface: {goal:?}"
        );
    }
}
