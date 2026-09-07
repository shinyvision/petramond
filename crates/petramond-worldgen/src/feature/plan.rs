use std::collections::BTreeMap;

use petramond_world::{block::Block, chunk::SECTION_SIZE, mathh::IVec3, section::Section};

use super::{FeatureCtx, PlacementRule, SectionSink, VoxelSink};

/// Ordered geometry operations for one column, partitioned by destination
/// section, recorded once and replayed into each section as it generates.
/// Predicates remain operations: baking final blocks here would lose
/// vegetation and mod writes that land before the replay.
pub(crate) struct FeaturePlan {
    sections: BTreeMap<i32, Vec<Placement>>,
}

struct Placement {
    pos: IVec3,
    block: Block,
    rule: PlacementRule,
}

impl FeaturePlan {
    /// Record whatever `features` writes through the context into the column
    /// `(cx, cz)`; writes outside that footprint are dropped exactly as a
    /// clipped sink drops them. Any origin loop — trees, a future scatter, a
    /// mod feature — is a valid source.
    pub(crate) fn record(cx: i32, cz: i32, features: impl FnOnce(&mut FeatureCtx)) -> Self {
        let mut recorder = Recorder {
            ox: cx * SECTION_SIZE as i32,
            oz: cz * SECTION_SIZE as i32,
            sections: BTreeMap::new(),
        };
        features(&mut FeatureCtx::new(&mut recorder));
        Self {
            sections: recorder.sections,
        }
    }

    pub(crate) fn apply(&self, section: &mut Section) {
        let cy = section.origin_world().1.div_euclid(SECTION_SIZE as i32);
        if let Some(placements) = self.sections.get(&cy) {
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
    ox: i32,
    oz: i32,
    sections: BTreeMap<i32, Vec<Placement>>,
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
        if (self.ox..self.ox + side).contains(&pos.x) && (self.oz..self.oz + side).contains(&pos.z)
        {
            self.sections
                .entry(pos.y.div_euclid(side))
                .or_default()
                .push(Placement { pos, block, rule });
        }
    }
}

#[cfg(test)]
mod tests;
