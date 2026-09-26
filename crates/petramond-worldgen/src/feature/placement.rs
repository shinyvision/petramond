use std::collections::BTreeSet;
use std::sync::Arc;

use petramond_world::{mathh::IVec3, section::Section};

use super::{Feature, FeaturePlan};
use crate::{rng::FeatureRng, TerrainSpace};

/// An admitted configured feature: its [`FeaturePlan`], recorded before
/// section clipping and replayed with the engine trees' own overwrite rules.
/// Occupancy and root support use positional terrain, so a neighbour cannot
/// admit half a tree whose trunk failed in its owner's section.
pub struct PlacedFeature {
    plan: FeaturePlan,
}

/// A memoized [`PlacedFeature::resolve`]: the world's context, the feature
/// key, the origin and the salt.
pub(crate) type PlacedKey = (crate::cache::GenContext, Box<str>, [i32; 3], u64);

/// Resident bytes of a memoized placement.
pub(crate) fn placed_heap(placed: &Arc<PlacedFeature>) -> usize {
    std::mem::size_of::<PlacedFeature>() + placed.plan.memory_bytes()
}

impl PlacedFeature {
    /// Choose the first complete fit. Candidate sets are replayed whole by every
    /// section; cached admission is independent of section order and residency.
    pub fn resolve_first(
        key: &str,
        origins: &[[i32; 3]],
        world_seed: u32,
        salt: u64,
    ) -> Result<Arc<Self>, String> {
        if origins.is_empty() || origins.len() > 16 {
            return Err("configured feature requires 1..=16 candidate origins".into());
        }
        if origins
            .iter()
            .flatten()
            .any(|n| n.unsigned_abs() > 30_000_000)
        {
            return Err("configured feature origin exceeds world bounds".into());
        }
        let caches = crate::cache::installed();
        let context = crate::cache::GenContext::installed(world_seed);
        Self::first_admitted(origins, |origin| {
            let memo_key: PlacedKey = (context, key.into(), origin, salt);
            if let Some(placed) = caches.terrain.placed_features.get(&memo_key) {
                return Ok(placed);
            }
            let placed = Arc::new(Self::resolve(key, origin, world_seed, salt)?);
            caches
                .terrain
                .placed_features
                .insert(memo_key, Arc::clone(&placed));
            Ok(placed)
        })
    }

    fn first_admitted(
        origins: &[[i32; 3]],
        mut resolve: impl FnMut([i32; 3]) -> Result<Arc<Self>, String>,
    ) -> Result<Arc<Self>, String> {
        for &origin in origins {
            let placed = resolve(origin)?;
            if !placed.plan.is_empty() {
                return Ok(placed);
            }
        }
        Ok(Arc::new(Self {
            plan: FeaturePlan::empty(),
        }))
    }

    /// Resolve and sample one configured feature. Occupied geometry or an
    /// unsupported ground cell yields an empty placement; invalid references
    /// and geometry outside the feature envelope are errors.
    pub fn resolve(
        key: &str,
        origin: [i32; 3],
        world_seed: u32,
        salt: u64,
    ) -> Result<Self, String> {
        let configured = crate::data::features::by_name(key)
            .ok_or_else(|| format!("unknown configured feature '{key}'"))?;
        if origin.iter().any(|n| n.unsigned_abs() > 30_000_000) {
            return Err("configured feature origin exceeds world bounds".into());
        }
        let rng = FeatureRng::positional(world_seed, salt, origin[0], origin[1], origin[2]);
        Self::record(configured.feature, origin.into(), rng, |probes| {
            crate::terrain_space_at(world_seed, probes)
        })
    }

    fn record(
        feature: &dyn Feature,
        origin: IVec3,
        mut rng: FeatureRng,
        solid: impl FnOnce(&[[i32; 3]]) -> Vec<TerrainSpace>,
    ) -> Result<Self, String> {
        let plan = FeaturePlan::record_feature(feature, origin, &mut rng)
            .ok_or("configured feature exceeds its placement envelope")?;
        // Every cell the feature writes must be open terrain, and every
        // non-leaf cell it roots at the origin's level must stand on ground.
        let cells: BTreeSet<[i32; 3]> = plan.placements().map(|p| p.pos.to_array()).collect();
        let ground: BTreeSet<[i32; 3]> = plan
            .placements()
            .filter(|p| p.pos.y == origin.y && !p.block.is_leaves())
            .map(|p| (p.pos - IVec3::Y).to_array())
            .collect();
        let mut probes: Vec<_> = cells
            .into_iter()
            .map(|p| (p, TerrainSpace::Air))
            .chain(ground.into_iter().map(|p| (p, TerrainSpace::Solid)))
            .collect();
        // Keep each terrain tile hot while probing an entire crown.
        probes.sort_unstable_by_key(|([x, y, z], _)| {
            (x.div_euclid(16), z.div_euclid(16), *x, *z, *y)
        });
        let positions: Vec<_> = probes.iter().map(|(p, _)| *p).collect();
        let answers = solid(&positions);
        if answers.len() != probes.len() {
            return Err("configured feature terrain query returned the wrong length".into());
        }
        let admitted = probes
            .iter()
            .zip(answers)
            .all(|((_, expected), actual)| *expected == actual);
        Ok(Self {
            plan: if admitted { plan } else { FeaturePlan::empty() },
        })
    }

    /// Whether admission rejected every candidate.
    pub fn is_empty(&self) -> bool {
        self.plan.is_empty()
    }

    pub(crate) fn apply(&self, section: &mut Section) {
        self.plan.apply(section);
    }
}

#[cfg(test)]
mod tests;
