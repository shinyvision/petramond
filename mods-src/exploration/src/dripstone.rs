use mod_sdk::*;

use crate::keys;

pub mod behavior;
pub mod gen;
pub mod hazard;

pub const BIOME_TOP_Y: i32 = 96;
/// Longest run growth builds. Worldgen places shorter ones.
pub const MAX_RUN: i32 = 7;
pub const PLACED_KEY: &str = "exploration:placed";

/// Which run a spike belongs to.
///
/// Wetness is a skin, not a run. The dry and dripping hanging rows share one authored `run` shape,
/// so the engine interns them to one shape kind and joins them into a single run. Every rule here
/// classifies by this, never by block id, or a wet tip under a dry segment would read as
/// unsupported and fall.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Run {
    Hanging,
    Standing,
}

impl Run {
    pub fn root_step(self) -> i32 {
        match self {
            Run::Hanging => 1,
            Run::Standing => -1,
        }
    }
}

pub struct Dripstone {
    pub block: BlockId,
    pub stalactite: BlockId,
    pub stalactite_wet: BlockId,
    pub stalagmite: BlockId,
    pub water: BlockId,
    pub fluids: crate::fluids::Fluids,
    pub air: BlockId,
    pub vessels: Vec<(BlockId, BlockId)>,
    pub biome: Option<u8>,
}

impl Dripstone {
    pub fn resolve(fluids: crate::fluids::Fluids) -> Option<Dripstone> {
        let mut vessels = Vec::new();
        for (vessel, spec) in blocks_with_data_as::<VesselSpec>(keys::DRIP_VESSEL) {
            if let Some(filled) = resolve_block_logged(&spec.filled) {
                vessels.push((vessel, filled));
            }
        }
        Some(Dripstone {
            block: resolve_block_logged(keys::DRIPSTONE_BLOCK)?,
            stalactite: resolve_block_logged(keys::STALACTITE)?,
            stalactite_wet: resolve_block_logged(keys::STALACTITE_WET)?,
            stalagmite: resolve_block_logged(keys::STALAGMITE)?,
            water: resolve_block_logged(keys::WATER)?,
            fluids,
            air: BlockId::AIR,
            vessels,
            biome: resolve_underground_biome(keys::DRIPSTONE_CAVES),
        })
    }

    pub fn is_spike(&self, b: BlockId) -> bool {
        self.run_of(b).is_some()
    }

    pub fn run_of(&self, b: BlockId) -> Option<Run> {
        if b == self.stalactite || b == self.stalactite_wet {
            Some(Run::Hanging)
        } else if b == self.stalagmite {
            Some(Run::Standing)
        } else {
            None
        }
    }

    pub fn segment_at(&self, pos: [i32; 3], run: Run) -> bool {
        get_block(pos).and_then(|b| self.run_of(b)) == Some(run)
    }

    pub fn vessel_fill(&self, vessel: BlockId) -> Option<BlockId> {
        self.vessels
            .iter()
            .find(|(v, _)| *v == vessel)
            .map(|(_, filled)| *filled)
    }

    pub fn run_root(&self, pos: [i32; 3], run: Run) -> (i32, [i32; 3]) {
        let step = run.root_step();
        let mut len = 1;
        let mut c = pos;
        for _ in 0..MAX_RUN {
            let s = [c[0], c[1] + step, c[2]];
            if !self.segment_at(s, run) {
                return (len, s);
            }
            c = s;
            len += 1;
        }
        (len, [c[0], c[1] + step, c[2]])
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct VesselSpec {
    filled: String,
}
