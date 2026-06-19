//! River carver.
//!
//! Produces a per-column carve plan: the valley floor the driver's fill pass cuts
//! down to, after which the column floods to sea level.
//!
//! ```text
//! bed   = max(sea - RIVER_BED_DEPTH, surf - RIVER_MAX_DEPTH)   // capped sub-sea bed
//! floor = surf - (surf - bed) * lateral * land_fade
//! ```
//!
//! The design problem: the base terrain spline puts inland "plains" well ABOVE sea
//! level (~sea+6..sea+12), but water only sits below the global sea level. A naive
//! "carve where wet" rule therefore makes rivers dry up at every minor rise and
//! break into short disconnected stubs. To read as real, long, meandering rivers
//! the channel must instead carve a continuous VALLEY down to a sub-sea bed:
//!
//! - **bed** is a flat bed `RIVER_BED_DEPTH` below sea (so the core ALWAYS floods —
//!   the channel runs as continuous water, not a chain of wet pinch points),
//!   clamped so it never sits more than `RIVER_MAX_DEPTH` below the surface (a
//!   river VALLEY, not an ever-deepening canyon).
//! - **lateral** `= smoothstep(RIVER_BANK, RIVER_CORE, strength)` — 1 across the
//!   channel core, tapering to 0 at the bank, so the floor slopes back up to the
//!   surface. Sets the channel/valley width. A low `RIVER_CORE` makes even
//!   off-centre / pinched stretches reach the bed, keeping the water continuous.
//! - **land_fade** `= 1 - smoothstep(sea+FADE_LO, sea+FADE_HI, surf)` — held at 1
//!   through the lowland the river runs along and only tapering near the
//!   eligibility cap, so the river ENDS by sloping gently back to the terrain (no
//!   cliff) where it climbs into upland, rather than fading at every gentle rise.
//!
//! Where the land rises past ~`sea + (RIVER_MAX_DEPTH - RIVER_BED_DEPTH)` the capped
//! bed lifts above sea, so the river runs dry and slopes out. These dry CROSSINGS /
//! endings are unavoidable given the high base terrain, but the wide valley keeps
//! them as gentle dry washes, not steep slot trenches. (Verify the wet/dry mix and
//! continuity with `genmap <seed> out.png river` + the hillshade `shade` mode.)
//!
//! Rivers only carve non-mountain lowland and gentle hills (`surf <= sea +
//! RIVER_MAX_LAND`); taller land is left alone so rivers thread valleys instead of
//! canyoning mountains.

use crate::chunk::SEA_LEVEL;
use crate::mathh::smoothstep;
use super::{CarvePlan, Carver};

/// Highest land (above sea) a river will still carve through. Land taller than
/// this is left alone so rivers thread valleys instead of canyoning mountains.
/// MUST stay well below the overhang-carve onset (`surf` ~96 in `driver.rs`): the
/// driver only applies the river cut on `amp == 0.0` columns and the feature pass
/// only skips trees above the y95 treeline, so an eligible river column must never
/// be a 3-D-carved mountain column. `sea + 18 == 82 < 96` keeps that true; raising
/// this past ~31 would let the carve, the driver gate, and the tree anchor drift.
const RIVER_MAX_LAND: i32 = 18;
/// Land height (above sea) where the river's END begins to slope back: only here,
/// near the eligibility cap, does the cut taper toward zero. Kept HIGH (not down in
/// the lowland the river runs along) so the channel stays continuous and wet across
/// rolling terrain instead of fragmenting into short stubs at every gentle rise.
const RIVER_FADE_LO: i32 = 10;
/// Land height (above sea) at which the river has fully ended — the cut is faded to
/// zero and the terrain is restored. Matched to `RIVER_MAX_LAND` so the channel
/// vanishes smoothly right at the hard eligibility cap, never an abrupt edge.
const RIVER_FADE_HI: i32 = 15;
/// Depth of the flat channel bed below sea level: the water depth over the core of
/// a lowland channel. Sub-sea so the core ALWAYS floods — the key to long, conti-
/// nuous, meandering rivers rather than a chain of disconnected wet pinch points.
const RIVER_BED_DEPTH: f32 = 4.0;
/// Cap on how far below the natural surface the bed can sit. In lowland the bed is
/// the sub-sea `sea - RIVER_BED_DEPTH`; once the land rises past ~`sea + (MAX_DEPTH
/// - BED_DEPTH)` this cap lifts the bed above sea and the river runs dry and ends.
/// Bounds the channel to a river valley, not an ever-deepening canyon.
const RIVER_MAX_DEPTH: f32 = 15.0;
/// River strength at/above which the channel is at full depth (the flat sub-sea
/// bed). Between `RIVER_BANK` and this the floor ramps from the surface down to
/// the bed, forming the sloped bank.
const RIVER_CORE: f32 = 0.50;
/// River strength at/below which there is no cut at all (natural terrain). The
/// bank slope spans `RIVER_BANK..RIVER_CORE`; widening this gap widens the river.
const RIVER_BANK: f32 = 0.10;

pub struct RiverCarver;

impl Carver for RiverCarver {
    #[inline]
    fn plan(&self, river: f32, surf: i32) -> CarvePlan {
        let eligible = river > 0.05 && surf <= SEA_LEVEL + RIVER_MAX_LAND;
        let sea = SEA_LEVEL as f32;
        // Channel bed: a flat bed RIVER_BED_DEPTH below sea (so the core always
        // floods and the river runs as continuous water), but never deeper than
        // RIVER_MAX_DEPTH below the natural surface. In lowland the bed is sub-sea;
        // only once the land climbs past ~sea + (MAX_DEPTH - BED_DEPTH) does the cap
        // lift it above sea, so the river runs dry and ENDS there (rather than going
        // dry at every minor rise — which fragmented rivers into short stubs).
        let bed = (sea - RIVER_BED_DEPTH).max(surf as f32 - RIVER_MAX_DEPTH);
        // Lateral profile: 1 across the channel core (flat bed -> the core ALWAYS
        // reaches the sub-sea bed, the dry-V fix: a meandering channel stays wet
        // instead of breaking up), sloping to 0 at the bank for the bank width.
        let lateral = smoothstep(RIVER_BANK, RIVER_CORE, river);
        // Slope-back at the END: taper the cut to 0 only as the land nears the
        // eligibility cap, so a river climbing into upland ends by sloping gently
        // back to the terrain instead of at a cliff. High window -> acts only at the
        // genuine end, leaving the lowland channel at full depth.
        let land_fade = 1.0
            - smoothstep(
                (SEA_LEVEL + RIVER_FADE_LO) as f32,
                (SEA_LEVEL + RIVER_FADE_HI) as f32,
                surf as f32,
            );
        // Floor interpolates the surface (no cut) down to the bed, scaled laterally
        // (banks) and by the end fade. A dry cut can only occur where the capped bed
        // sits above sea — i.e. on rising ground at the river's end, sloping back.
        let floor = surf as f32 - (surf as f32 - bed) * lateral * land_fade;
        CarvePlan {
            carve: eligible,
            river_floor: (floor.round() as i32).min(surf),
        }
    }
}
