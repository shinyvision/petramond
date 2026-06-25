//! Wander: the idle-roaming behavior.
//!
//! Each game tick, while the mob has no active path, there's a small chance it picks
//! a new random destination — a standable foothold within a radius — and hands it to
//! the navigator. While the navigator is still walking the mob there, wander simply
//! keeps requesting that same destination (so the brain doesn't repath every tick).
//! When the mob arrives (or the navigator gives up), wander goes quiet until its next
//! random roll. The destination is always a real foothold, so the navigator can
//! actually path to it (or to the closest reachable cell).
//!
//! Destinations are filtered by the species' [`Habitat`]: avoided biomes are never
//! targeted (bar a bounded escape hatch so a hemmed-in mob still moves), and among
//! the rest preferred biomes win out — so, e.g., an owl hugs forest and drifts back
//! toward it after straying.

use crate::biome::Biome;
use crate::block::Block;
use crate::mathh::IVec3;

use super::super::brain::{AiBehavior, AiCtx, BehaviorOutput};
use super::super::path::is_foothold;
use super::super::Habitat;

/// How many random offsets to try when picking a destination before giving up for
/// this tick (keeps the search cheap; it only runs on the occasional roll).
const PICK_ATTEMPTS: u32 = 24;

/// After this many avoided-biome candidates have been passed over in one pick, the
/// avoid rule lifts for the rest of that pick — so a mob boxed in by avoided terrain
/// isn't frozen, it just settles for the best it can reach.
const AVOID_ESCAPE: u32 = 5;

/// Same idea for water (for a water-averse species): re-roll a water destination this
/// many times, then accept a wet one rather than refuse to move. Crossing water on
/// the way to a dry destination is unaffected — that's the pathfinder's call.
const WATER_ESCAPE: u32 = 3;

pub struct WanderAi {
    chance_per_tick: f32,
    radius: i32,
    /// The species' biome affinity, consulted when choosing a destination.
    habitat: &'static Habitat,
    /// Whether to steer destinations away from water (with the bounded re-roll above).
    avoid_water: bool,
    /// The destination currently being walked to (if any).
    current: Option<IVec3>,
}

impl WanderAi {
    pub fn new(
        chance_per_tick: f32,
        radius: i32,
        habitat: &'static Habitat,
        avoid_water: bool,
    ) -> Self {
        WanderAi {
            chance_per_tick,
            radius,
            habitat,
            avoid_water,
            current: None,
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
            if ctx.rng.next_f32() < self.chance_per_tick {
                self.current = pick_destination(ctx, self.radius, self.habitat, self.avoid_water);
            }
            self.current
        };
        BehaviorOutput {
            goal,
            ..Default::default()
        }
    }
}

/// Pick a random standable destination within `radius` of the mob, honoring the
/// `habitat` (see [`Picker`]), or `None` if nothing suitable turned up in a few
/// tries. Samples a horizontal offset (inside the radius), classifies that column's
/// biome, and — for columns that clear the avoid filter — finds the foothold in the
/// column nearest the mob's level.
fn pick_destination(
    ctx: &mut AiCtx,
    radius: i32,
    habitat: &Habitat,
    avoid_water: bool,
) -> Option<IVec3> {
    let solid = |c: IVec3| Block::from_id(ctx.world.chunk_block(c.x, c.y, c.z)).blocks_movement();
    let water = |c: IVec3| Block::from_id(ctx.world.chunk_block(c.x, c.y, c.z)).is_water();
    let r2 = radius * radius;
    let mut picker = Picker::new(AVOID_ESCAPE, WATER_ESCAPE);
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
        let Some(y) = nearest_foothold_y(x, z, ctx.cell.y, radius, ctx.head, &solid) else {
            continue;
        };
        let dest = IVec3::new(x, y, z);
        // For a water-averse species, re-roll a destination that sits in water — up to
        // the escape hatch, after which a wet spot is accepted rather than refusing.
        if avoid_water && picker.reject_water(water(dest)) {
            continue;
        }
        if let Some(dest) = picker.offer(dest, fit) {
            return Some(dest);
        }
    }
    // No preferred foothold turned up: fall back to the first allowed one we saw (a
    // neutral biome, or — once an escape hatch tripped — an avoided / wet one).
    picker.into_fallback()
}

/// The foothold Y in column `(x, z)` closest to `y0`, scanning outward up to
/// `radius` cells either way, or `None` if the column has no foothold in range.
fn nearest_foothold_y(
    x: i32,
    z: i32,
    y0: i32,
    radius: i32,
    head: i32,
    solid: &impl Fn(IVec3) -> bool,
) -> Option<i32> {
    for d in 0..=radius {
        for y in [y0 - d, y0 + d] {
            if is_foothold(IVec3::new(x, y, z), head, solid) {
                return Some(y);
            }
        }
    }
    None
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
    use super::*;

    fn habitat() -> Habitat {
        Habitat {
            avoid: &[Biome::Plains, Biome::Desert],
            prefer: &[Biome::Forest],
        }
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
}
