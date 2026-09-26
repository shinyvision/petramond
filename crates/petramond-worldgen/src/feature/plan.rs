use std::collections::BTreeMap;

use petramond_world::{block::Block, chunk::SECTION_SIZE, mathh::IVec3, section::Section};

use super::{Feature, FeatureCtx, PlacementRule, SectionSink, VoxelSink};
use crate::rng::FeatureRng;

/// Ordered feature geometry operations, partitioned by destination section,
/// recorded once and replayed into each section as it generates. THE one
/// feature recorder: engine trees (one column's origins) and mod-placed
/// configured features (one admitted origin) both record through it and
/// replay with the same overwrite rules, so a tree behaves the same whoever
/// placed it. Predicates remain operations: baking final blocks here would
/// lose vegetation and mod writes that land before the replay.
pub(crate) struct FeaturePlan {
    sections: BTreeMap<[i32; 3], Vec<Placement>>,
}

pub(crate) struct Placement {
    pub(crate) pos: IVec3,
    pub(crate) block: Block,
    pub(crate) rule: PlacementRule,
}

/// Which writes a recording keeps.
enum Bounds {
    /// Only writes inside column `(ox, oz)`'s footprint; the rest are dropped
    /// exactly as a clipped sink drops them.
    Column { ox: i32, oz: i32 },
    /// Every write within the feature envelope around `origin` (the replay
    /// margin horizontally, the tree reach upward); one outside it spoils the
    /// recording.
    Envelope { origin: IVec3, overflow: bool },
}

impl FeaturePlan {
    /// Record whatever `features` writes through the context into the column
    /// `(cx, cz)`. Any origin loop — trees, a future scatter, a mod feature —
    /// is a valid source.
    pub(crate) fn record(cx: i32, cz: i32, features: impl FnOnce(&mut FeatureCtx)) -> Self {
        let side = SECTION_SIZE as i32;
        let mut recorder = Recorder {
            bounds: Bounds::Column {
                ox: cx * side,
                oz: cz * side,
            },
            sections: BTreeMap::new(),
        };
        features(&mut FeatureCtx::new(&mut recorder));
        Self {
            sections: recorder.sections,
        }
    }

    /// Record one feature generated at `origin`, unclipped. `None` when it
    /// writes outside its envelope — geometry no section's replay margin
    /// covers.
    pub(crate) fn record_feature(
        feature: &dyn Feature,
        origin: IVec3,
        rng: &mut FeatureRng,
    ) -> Option<Self> {
        let mut recorder = Recorder {
            bounds: Bounds::Envelope {
                origin,
                overflow: false,
            },
            sections: BTreeMap::new(),
        };
        feature.generate(
            &mut FeatureCtx::new(&mut recorder),
            &mut |_| true,
            origin,
            rng,
        );
        match recorder.bounds {
            Bounds::Envelope { overflow: true, .. } => None,
            _ => Some(Self {
                sections: recorder.sections,
            }),
        }
    }

    /// A plan that writes nothing.
    pub(crate) fn empty() -> Self {
        Self {
            sections: BTreeMap::new(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }

    /// Every recorded operation, in section then write order.
    pub(crate) fn placements(&self) -> impl Iterator<Item = &Placement> {
        self.sections.values().flatten()
    }

    pub(crate) fn apply(&self, section: &mut Section) {
        let (ox, oy, oz) = section.origin_world();
        let side = SECTION_SIZE as i32;
        let key = [
            ox.div_euclid(side),
            oy.div_euclid(side),
            oz.div_euclid(side),
        ];
        if let Some(placements) = self.sections.get(&key) {
            let mut sink = SectionSink::new(section);
            for p in placements {
                sink.place(p.pos, p.block, p.rule);
            }
        }
    }

    pub(crate) fn memory_bytes(&self) -> usize {
        self.sections
            .values()
            .map(|p| p.capacity() * std::mem::size_of::<Placement>())
            .sum()
    }
}

struct Recorder {
    bounds: Bounds,
    sections: BTreeMap<[i32; 3], Vec<Placement>>,
}

impl VoxelSink for Recorder {
    /// A plan has no destination yet, so every cell reads as the sink
    /// contract's unaddressable value; predicates are kept as rules and
    /// evaluated against the real section at replay.
    fn get(&self, _: IVec3) -> Block {
        Block::Air
    }

    fn set(&mut self, p: IVec3, b: Block) {
        self.place(p, b, PlacementRule::Always);
    }

    fn place(&mut self, pos: IVec3, block: Block, rule: PlacementRule) {
        let side = SECTION_SIZE as i32;
        let keep = match &mut self.bounds {
            Bounds::Column { ox, oz } => {
                (*ox..*ox + side).contains(&pos.x) && (*oz..*oz + side).contains(&pos.z)
            }
            Bounds::Envelope { origin, overflow } => {
                let delta = pos - *origin;
                let inside = delta.x.abs() <= super::MARGIN
                    && delta.z.abs() <= super::MARGIN
                    && (0..=super::MAX_TREE_REACH_ABOVE).contains(&delta.y);
                *overflow |= !inside;
                inside
            }
        };
        if keep {
            let section = [
                pos.x.div_euclid(side),
                pos.y.div_euclid(side),
                pos.z.div_euclid(side),
            ];
            self.sections
                .entry(section)
                .or_default()
                .push(Placement { pos, block, rule });
        }
    }
}

#[cfg(test)]
mod tests;
