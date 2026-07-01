//! Shared surface-rule building blocks selected by individual biome modules.

use crate::block::Block;
use crate::worldgen::surface::rule::{SurfaceCond, SurfaceRule};

const REDWOOD_GRASS_SALT: u64 = 0x0000_5245_4457_0047;

pub(super) static PLAINS_TOP: SurfaceRule = SurfaceRule::Sequence(&[
    SurfaceRule::Condition {
        when: SurfaceCond::DepthFromTop(0),
        then: &SurfaceRule::Block(Block::Grass),
    },
    SurfaceRule::Condition {
        when: SurfaceCond::DepthFromTop(3),
        then: &SurfaceRule::Block(Block::Dirt),
    },
    SurfaceRule::Block(Block::Stone),
]);

pub(super) static FOOTHILLS_TOP: SurfaceRule = SurfaceRule::Sequence(&[
    SurfaceRule::Condition {
        when: SurfaceCond::DepthFromTop(0),
        then: &SurfaceRule::Block(Block::Grass),
    },
    SurfaceRule::Condition {
        when: SurfaceCond::DepthFromTop(2),
        then: &SurfaceRule::Block(Block::Dirt),
    },
    SurfaceRule::Block(Block::Stone),
]);

static SNOW_CAP: SurfaceRule = SurfaceRule::Sequence(&[
    SurfaceRule::Condition {
        when: SurfaceCond::DepthFromTop(0),
        then: &SurfaceRule::Block(Block::Snow),
    },
    SurfaceRule::Block(Block::Stone),
]);

pub(super) static MOUNTAIN_TOP: SurfaceRule = SurfaceRule::Sequence(&[
    SurfaceRule::Condition {
        when: SurfaceCond::SurfaceAboveY(143),
        then: &SNOW_CAP,
    },
    SurfaceRule::Condition {
        when: SurfaceCond::SurfaceAboveY(135),
        then: &SurfaceRule::Block(Block::Stone),
    },
    SurfaceRule::Condition {
        when: SurfaceCond::DepthFromTop(0),
        then: &SurfaceRule::Block(Block::Grass),
    },
    SurfaceRule::Condition {
        when: SurfaceCond::DepthFromTop(2),
        then: &SurfaceRule::Block(Block::Dirt),
    },
    SurfaceRule::Block(Block::Stone),
]);

pub(super) static SNOW_TOP: SurfaceRule = SurfaceRule::Sequence(&[
    SurfaceRule::Condition {
        when: SurfaceCond::DepthFromTop(1),
        then: &SurfaceRule::Block(Block::Snow),
    },
    SurfaceRule::Block(Block::Stone),
]);

pub(super) static SAND_DEEP: SurfaceRule = SurfaceRule::Sequence(&[
    SurfaceRule::Condition {
        when: SurfaceCond::DepthFromTop(4),
        then: &SurfaceRule::Block(Block::Sand),
    },
    SurfaceRule::Block(Block::Stone),
]);

pub(super) static OCEAN_FLOOR: SurfaceRule = SurfaceRule::Sequence(&[
    SurfaceRule::Condition {
        when: SurfaceCond::DepthFromTop(2),
        then: &SurfaceRule::Block(Block::Sand),
    },
    SurfaceRule::Condition {
        when: SurfaceCond::DepthFromTop(4),
        then: &SurfaceRule::Block(Block::Dirt),
    },
    SurfaceRule::Block(Block::Stone),
]);

pub(super) static DEEP_OCEAN_FLOOR: SurfaceRule = SurfaceRule::Sequence(&[
    SurfaceRule::Condition {
        when: SurfaceCond::DepthFromTop(2),
        then: &SurfaceRule::Block(Block::Dirt),
    },
    SurfaceRule::Block(Block::Stone),
]);

// `Underwater` holds for EVERY voxel of a below-sea-level column, so an
// underwater floor material must also be depth-gated — an ungated branch would
// paint the column to bedrock and cave carving would expose it (all-sand caves
// under swamp lakes).
static UNDERWATER_DIRT_BAND: SurfaceRule = SurfaceRule::Condition {
    when: SurfaceCond::DepthFromTop(3),
    then: &SurfaceRule::Block(Block::Dirt),
};

static UNDERWATER_SAND_BAND: SurfaceRule = SurfaceRule::Condition {
    when: SurfaceCond::DepthFromTop(2),
    then: &SurfaceRule::Block(Block::Sand),
};

pub(super) static PODZOL_TOP: SurfaceRule = SurfaceRule::Sequence(&[
    SurfaceRule::Condition {
        when: SurfaceCond::Underwater,
        then: &UNDERWATER_DIRT_BAND,
    },
    SurfaceRule::Condition {
        when: SurfaceCond::DepthFromTop(0),
        then: &SurfaceRule::Block(Block::Podzol),
    },
    SurfaceRule::Condition {
        when: SurfaceCond::DepthFromTop(3),
        then: &SurfaceRule::Block(Block::Dirt),
    },
    SurfaceRule::Block(Block::Stone),
]);

static STONY_CALCITE_CAP: SurfaceRule = SurfaceRule::Sequence(&[
    SurfaceRule::Condition {
        when: SurfaceCond::DepthFromTop(0),
        then: &SurfaceRule::Block(Block::Calcite),
    },
    SurfaceRule::Block(Block::Stone),
]);

pub(super) static STONY_TOP: SurfaceRule = SurfaceRule::Sequence(&[
    SurfaceRule::Condition {
        when: SurfaceCond::SurfaceAboveY(150),
        then: &STONY_CALCITE_CAP,
    },
    SurfaceRule::Block(Block::Stone),
]);

pub(super) static WETLAND_TOP: SurfaceRule = SurfaceRule::Sequence(&[
    SurfaceRule::Condition {
        when: SurfaceCond::Underwater,
        then: &UNDERWATER_SAND_BAND,
    },
    SurfaceRule::Condition {
        when: SurfaceCond::DepthFromTop(0),
        then: &SurfaceRule::Block(Block::Grass),
    },
    SurfaceRule::Condition {
        when: SurfaceCond::DepthFromTop(3),
        then: &SurfaceRule::Block(Block::Dirt),
    },
    SurfaceRule::Block(Block::Stone),
]);

// Podzol floor with the OCCASIONAL grass CLUSTER: a smooth low-frequency field
// cuts out contiguous grass patches instead of the per-column speckle a white-noise
// hash gives, so the floor reads as podzol dappled with natural grass clumps.
static REDWOOD_CAP: SurfaceRule = SurfaceRule::Sequence(&[
    SurfaceRule::Condition {
        when: SurfaceCond::ClusterNoiseBelow {
            salt: REDWOOD_GRASS_SALT,
            threshold: 0.30,
            period: 7.0,
        },
        then: &SurfaceRule::Block(Block::Grass),
    },
    SurfaceRule::Block(Block::Podzol),
]);

pub(super) static REDWOOD_TOP: SurfaceRule = SurfaceRule::Sequence(&[
    SurfaceRule::Condition {
        when: SurfaceCond::Underwater,
        then: &UNDERWATER_DIRT_BAND,
    },
    SurfaceRule::Condition {
        when: SurfaceCond::DepthFromTop(0),
        then: &REDWOOD_CAP,
    },
    SurfaceRule::Condition {
        when: SurfaceCond::DepthFromTop(3),
        then: &SurfaceRule::Block(Block::Dirt),
    },
    SurfaceRule::Block(Block::Stone),
]);
