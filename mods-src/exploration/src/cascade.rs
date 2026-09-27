//! Cascades: long rimstone basins traced ALONG the cave floor's own contours,
//! each spilling over its lip and down the fall line into the basin below.
//!
//! # The model
//!
//! Real rimstone terraces form along contour lines — a long sinuous lip
//! holding one water level across a slope — and the water pours over the lip
//! down the fall lines into the next terrace. This module builds exactly that:
//!
//! 1. A coarse height scan of the candidate's lattice cell finds STEP EDGES —
//!    places where the cave floor drops — and chains them into contours: runs
//!    of edge samples whose lip height drifts gradually along their length.
//!    The longest contour is the site. There is no shape to fit and no centre
//!    to roll; the terrain's own edge IS the design.
//! 2. The head basin floods the terrace behind the highest point of that
//!    contour: a region grow over columns whose floor lies within a couple of
//!    blocks under the surface, so on rolling ground the basin is a long band
//!    following the contour, wide where the ground is gentle and narrow where
//!    it steepens. Nothing is carved; the bed is the floor the carver made,
//!    lined with one course of silt, adopting natural pits as deep spots.
//! 3. Where the floor steps down past the basin's own depth band, a new basin
//!    grows on the lower terrace, seeded from the highest floor just past the
//!    lip — down the fall line, or further along a contour that keeps rolling
//!    downward. Every link gets one spill notch through the lip, and the pour
//!    is over a weir face into the pool below.
//! 4. Containment is natural rock where the terrain rises, and placed silt
//!    dams — rimstone — where it does not. A rim column that cannot be sealed
//!    within the dam budget does not reject the basin: the water RETREATS from
//!    that edge and the seal runs again. Rejection is reserved for terrain
//!    that offers no descent at all.
//!
//! # Cascades are TERRAIN; giants adapt
//!
//! A basin is part of the ground; a mushroom is something that grows on the
//! ground. The basin is therefore resolved FIRST, from the terrain alone, and
//! is never rejected because of a giant. Giants adapt to it positionally,
//! and none stands in the water: one whose body would sit in a pool, float on
//! its surface, or break the containment proof (blocking a spill,
//! intercepting a fall) is SUPPRESSED — the feature carries the suppressed
//! anchors and the giant pass skips them, in every section, because the whole
//! decision is a pure function of `(seed, cell)`.
//!
//! # The containment argument
//!
//! Water is a ticking fluid and worldgen schedules no flow check, so "it did
//! not flood when it generated" is evidence of nothing. What must hold is the
//! footprint invariant: water may flow, fall and pool freely INSIDE the
//! feature, and must never reach the ordinary cavern floor outside it.
//! Because the basin adopts cave air, that cannot be proved by construction;
//! it is proved by exhaustion: [`Trace::build`] computes the water's whole
//! REACHABLE SET over the final geometry — probed terrain, minus cut notch
//! cells, plus placed silt, plus every standing giant body — under a
//! conservative model of the fluid sim. One step past the probed domain
//! rejects; every pool must actually receive inflow or the chain is scenery.
//!
//! The model leans on four facts of `crates/petramond-world/src/fluid.rs`,
//! all load-bearing: water never moves upward; a falling cell pours straight
//! down and never spreads sideways while it can; a flowing cell suspended
//! over water never spreads sideways (only sources spread across the top of
//! water); and a fall landing in source water stops dead, while one landing
//! on solid spreads a full-strength ring. Everywhere else the model
//! over-approximates.
//!
//! # Which read may decide
//!
//! A cascade spans sections, so accept/reject must be unanimous. Every input
//! is positional: the rarity roll is `GenRng::positional` on lattice
//! coordinates, every terrain read goes through [`crate::probe`], and the
//! giants folded into the model are re-derived from their own positional
//! rolls. Nothing consults the dispatching section's snapshot, so the
//! sections a cascade straddles cannot disagree about whether it exists —
//! which for water is not a cosmetic seam but a hole in a dam. Everything in
//! this module is pure: the caller asks the host, this module decides.
//!
//! # Layout
//!
//! One module per stage, each a pure function of its inputs: `site` (roll,
//! coarse scan, contour traces, band plan), `basin` (column classification,
//! chain growth, pit adoption), `seal` (dams and the retreat), `notch`
//! (spill channels), `flood` (the containment proof), `build` (the stage
//! pipeline), `finish` (giants folded in, feature assembled) and `codec`
//! (the memo encoding).

mod basin;
mod build;
mod codec;
mod finish;
mod flood;
mod notch;
mod seal;
mod site;

use std::collections::BTreeSet;

pub use build::Built;
pub use codec::memo_key;
pub use site::{cells_overlapping, cells_overlapping_box, Cell, Trace};

/// Named positional-RNG salt keeps cascade site rolls independent from other
/// worldgen streams.
const SALT_CASCADE: u64 = mod_sdk::GenRng::salt("exploration:cascade");

/// A candidate's whole footprint — pools, dams, probe shell — is confined to
/// its own lattice cell, horizontally and vertically. That confinement is a
/// containment argument, not a tidiness one: two cascades that could overlap
/// could each open a cell the other's proof holds solid, and neither would
/// ever know. Disjoint cells make the case impossible instead of rare.
///
/// The cell is 96 wide because the cell WALL is where a basin is forced to
/// stop regardless of terrain. A 96-cell span lets long contour edges fit;
/// the vertical 32 covers a descending chain.
pub const LATTICE: i32 = 96;
pub const LATTICE_Y: i32 = 32;
/// Rarity is a ROLL, not a residue of rejections — this is THE frequency
/// lever, chosen deliberately: every biome cell with real relief carries a
/// cascade. The mushroom cavern is itself the rare event, and inside one the
/// water should be dependable, not a lottery; the gates below reject only
/// terrain with no descent, which in a carved cavern is the exception.
const ONE_IN: i32 = 1;
/// Footprint keeps this many columns inside the cell walls (dam + probe room).
const MARGIN: i32 = 2;
const COARSE: i32 = 4;
const NSAMP: i32 = (LATTICE - 2 * MARGIN - 2) / COARSE + 1;
const HEADROOM: i32 = 3;
/// How far under its surface a column's floor may lie and still belong to the
/// basin naturally. Small on purpose: it is what splits rolling ground into
/// TERRACES instead of drowning it under one deep sheet, and in-basin relief
/// within the band is the shelving of the bed.
const BED_BAND: i32 = 2;
const ADOPT_MAX: i32 = 5;
const MAX_STEP: i32 = 6;
pub const MAX_POOLS: usize = 6;
const MIN_POOLS: usize = 2;
const MIN_POOL_AREA: usize = 8;
/// Contour chains attempted per rolled cell, best-scored first. The first
/// that builds owns the cell — the cell mutex that keeps footprints disjoint.
/// Four, because a cell whose longest lip fronts a chasm (a wall, not a lip)
/// often carries a humbler edge that holds a fine terrace: more tries cost
/// nothing on cells that site early and rescue cells that would get nothing.
const EDGE_TRIES: usize = 4;
const CHAIN_MAX_SAMPLES: usize = 44;
const GROW_DILATE: i32 = 4;
const PROBE_DILATE: i32 = GROW_DILATE + 8;
const DAM_MAX: i32 = 6;
const DAM_TALL: i32 = 4;
const DAM_TALL_SHARE_MAX: usize = 50;
const OPEN_PERCENT: usize = 60;
const NOTCH_PATH_MAX: usize = 6;

#[cfg(test)]
pub const COARSE_PROBE: usize = (NSAMP * NSAMP) as usize * (LATTICE_Y as usize - 2);
pub const BAND_PROBE_MAX: usize = 24 * 4096;

const SIDES: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Kind {
    Water,
    Silt,
    Air,
}

pub struct Intruder {
    pub key: [i32; 3],
    pub root: [i32; 3],
    pub solid: BTreeSet<[i32; 3]>,
}

pub struct Feature {
    pub writes: Vec<([i32; 3], Kind)>,
    pub reserves: Vec<[i32; 3]>,
    pub suppressed: Vec<[i32; 3]>,
    #[cfg(test)]
    pub wet: Vec<[i32; 3]>,
}

#[cfg(test)]
mod tests;
