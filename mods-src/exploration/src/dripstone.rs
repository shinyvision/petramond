//! The dripstone caves: a dry, warm cave habitat lined with dripstone and
//! grown over with pointed dripstone — stalactites that drip, grow, fall and
//! impale; stalagmites that grow under them and spike whatever lands on them.
//!
//! The habitat is a row in `underground_biomes.json`; the spikes are two
//! `run` box-set rows the ENGINE shapes and refines (a cell's taper follows
//! its place in the run, placement picks hanging or standing from the click).
//! This module is the pack's POLICY over them — what a spike does, never
//! what it looks like:
//!
//! - [`behavior`]: the block hooks. A run that loses its root comes down
//!   (a stalactite as falling pieces, a stalagmite as drops); a dripping tip
//!   grows, grows a stalagmite under itself, or fills a vessel.
//! - [`hazard`]: a falling piece damages what it lands on; a fall onto a
//!   stalagmite hurts twice.
//! - [`gen`]: the worldgen dressing — runs off the ceilings and floors of the
//!   habitat, clustered into formations.

use mod_sdk::*;

pub mod behavior;
pub mod gen;
pub mod hazard;

/// The underground biome this pack registers for the caves.
pub const BIOME_KEY: &str = "exploration:dripstone_caves";
/// Top of the depth band that row declares (`"y": [-64, 96]`); worldgen
/// derives its altitude gate from here.
pub const BIOME_TOP_Y: i32 = 96;
/// Registry name of the spike item — what a falling piece IS in flight.
pub const POINTED_ITEM: &str = "exploration:pointed_dripstone";
/// Longest run growth builds. Worldgen places shorter ones.
pub const MAX_RUN: i32 = 7;
/// Cell KV marking a spike as PLAYER-PLACED.
///
/// Growth reads it, so a naturally generated formation never changes shape:
/// the caves keep the form they generated with, and a dripstone farm is
/// something a player BUILDS. Written from `block_placed`, which the server
/// pushes only on the placement path — a worldgen write never reaches it —
/// and carried onto every cell growth adds, so a cultivated run keeps
/// growing from its new tip. A block write clears the cell's KV, so the
/// mark dies with the spike and a cell that falls and is regenerated comes
/// back unmarked.
pub const PLACED_KEY: &str = "exploration:placed";

/// Item-data key a block row declares to be FILLED by a drip
/// (`{"filled": "<block name>"}` — the row the vessel becomes). The
/// furniture cauldron opts in through this pack's integration overlay.
const VESSEL_KEY: &str = "exploration:drip_vessel";

/// Which run a spike belongs to.
///
/// WETNESS IS A SKIN, not a run: the dry and the dripping hanging rows share
/// one authored `run` shape, so the engine interns them to one shape kind and
/// joins them into a single run — and every rule here classifies by this,
/// never by a block id, or a wet tip under a dry segment would read as
/// unsupported and fall.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Run {
    /// Hangs from the cell above (a stalactite).
    Hanging,
    /// Stands on the cell below (a stalagmite).
    Standing,
}

impl Run {
    /// Vertical step from a segment toward its ROOT.
    pub fn root_step(self) -> i32 {
        match self {
            Run::Hanging => 1,
            Run::Standing => -1,
        }
    }
}

/// The pack's dripstone ids, resolved once at init.
pub struct Dripstone {
    pub block: BlockId,
    pub stalactite: BlockId,
    /// The hanging row that CARRIES THE DRIP EMITTER. A tip wears it exactly
    /// while its run hangs from a dripstone block under water, so the drip a
    /// player sees means "this one is live"; the emitter's own
    /// `requires_open: below` keeps it to the free end, so a buried wet
    /// segment shows nothing.
    pub stalactite_wet: BlockId,
    pub stalagmite: BlockId,
    /// The engine's water source — what must stand over a stalactite's root
    /// block for the run to drip and grow.
    pub water: BlockId,
    pub air: BlockId,
    /// Vessel row → the row a drip fills it into.
    pub vessels: Vec<(BlockId, BlockId)>,
    pub biome: Option<u8>,
}

impl Dripstone {
    pub fn resolve() -> Option<Dripstone> {
        let mut vessels = Vec::new();
        for (vessel, raw) in blocks_with_data(VESSEL_KEY) {
            let Ok(spec) = serde_json::from_str::<VesselSpec>(&raw) else {
                log(&format!("exploration: malformed {VESSEL_KEY} entry: {raw}"));
                continue;
            };
            if let Some(filled) = resolve_block_logged(&spec.filled) {
                vessels.push((vessel, filled));
            }
        }
        Some(Dripstone {
            block: resolve_block_logged("exploration:dripstone_block")?,
            stalactite: resolve_block_logged("exploration:stalactite")?,
            stalactite_wet: resolve_block_logged("exploration:stalactite_wet")?,
            stalagmite: resolve_block_logged("exploration:stalagmite")?,
            water: resolve_block_logged("petramond:water")?,
            air: BlockId(0),
            vessels,
            biome: resolve_underground_biome(BIOME_KEY),
        })
    }

    pub fn is_spike(&self, b: BlockId) -> bool {
        self.run_of(b).is_some()
    }

    /// Which run `b` is a segment of, if any — the ONE classification every
    /// rule reads, so the dry and wet hanging rows are one run.
    pub fn run_of(&self, b: BlockId) -> Option<Run> {
        if b == self.stalactite || b == self.stalactite_wet {
            Some(Run::Hanging)
        } else if b == self.stalagmite {
            Some(Run::Standing)
        } else {
            None
        }
    }

    /// Whether the cell at `pos` holds a segment of `run`. An unreadable
    /// (unloaded) cell answers `false`; callers that must not act on missing
    /// information check for that themselves.
    pub fn segment_at(&self, pos: [i32; 3], run: Run) -> bool {
        get_block(pos).and_then(|b| self.run_of(b)) == Some(run)
    }

    /// The row a drip fills `vessel` into, if it is one.
    pub fn vessel_fill(&self, vessel: BlockId) -> Option<BlockId> {
        self.vessels
            .iter()
            .find(|(v, _)| *v == vessel)
            .map(|(_, filled)| *filled)
    }

    /// The run's length counted from `pos` toward its root, and the cell
    /// beyond its last segment — what holds the run up.
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
struct VesselSpec {
    filled: String,
}
