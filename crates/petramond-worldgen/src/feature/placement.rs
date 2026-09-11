use std::collections::BTreeMap;
use std::sync::Arc;

use petramond_world::{block::Block, mathh::IVec3, section::Section};

use super::{Feature, FeatureCtx, SectionSink, VoxelSink};
use crate::{rng::FeatureRng, TerrainSpace};

mod cache;

/// An admitted configured feature, recorded before section clipping. Occupancy
/// and root support use positional terrain, so a neighbour cannot admit half a
/// tree whose trunk failed in its owner's section.
pub struct PlacedFeature {
    cells: Vec<(IVec3, Block)>,
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
        Self::first_admitted(origins, |origin| {
            cache::resolve(key, origin, world_seed, salt)
        })
    }

    fn first_admitted(
        origins: &[[i32; 3]],
        mut resolve: impl FnMut([i32; 3]) -> Result<Arc<Self>, String>,
    ) -> Result<Arc<Self>, String> {
        for &origin in origins {
            let placed = resolve(origin)?;
            if !placed.cells.is_empty() {
                return Ok(placed);
            }
        }
        Ok(Arc::new(Self { cells: Vec::new() }))
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
        let mut recorder = Recorder {
            origin,
            cells: BTreeMap::new(),
            overflow: false,
        };
        feature.generate(
            &mut FeatureCtx::new(&mut recorder),
            &mut |_| true,
            origin,
            &mut rng,
        );
        if recorder.overflow {
            return Err("configured feature exceeds its placement envelope".into());
        }
        let cells: Vec<_> = recorder
            .cells
            .into_iter()
            .map(|(p, block)| (IVec3::from(p), block))
            .collect();
        let mut probes: Vec<_> = cells
            .iter()
            .map(|(p, _)| (p.to_array(), TerrainSpace::Air))
            .chain(
                cells
                    .iter()
                    .filter(|(p, block)| p.y == origin.y && !block.is_leaves())
                    .map(|(p, _)| ((*p - IVec3::Y).to_array(), TerrainSpace::Solid)),
            )
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
            cells: if admitted { cells } else { Vec::new() },
        })
    }

    pub(crate) fn apply(&self, section: &mut Section) {
        let mut sink = SectionSink::new(section);
        for &(pos, block) in &self.cells {
            // Earlier feature stages may have dressed the admitted terrain.
            // Their plants yield, while authored buildings remain intact.
            let current = sink.get(pos);
            if current == Block::Air || current.is_fragile() || current.is_leaves() {
                sink.set(pos, block);
            }
        }
    }
}

struct Recorder {
    origin: IVec3,
    cells: BTreeMap<[i32; 3], Block>,
    overflow: bool,
}

impl VoxelSink for Recorder {
    fn get(&self, pos: IVec3) -> Block {
        self.cells
            .get(&pos.to_array())
            .copied()
            .unwrap_or(Block::Air)
    }

    fn set(&mut self, pos: IVec3, block: Block) {
        let delta = pos - self.origin;
        if delta.x.abs() > crate::proto::MARGIN
            || delta.z.abs() > crate::proto::MARGIN
            || !(0..=super::MAX_TREE_REACH_ABOVE).contains(&delta.y)
        {
            self.overflow = true;
            return;
        }
        self.cells.insert(pos.to_array(), block);
    }
}

#[cfg(test)]
mod tests;
