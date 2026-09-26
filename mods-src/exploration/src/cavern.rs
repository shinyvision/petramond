//! Mushroom cavern dressing. Geometry comes from the separate excavation catalog.
//! Every cross-section placement decision reads positional terrain so caps,
//! stems and hanging growth agree regardless of section generation order —
//! the seam contract, the rule for which reads may decide, and the host-call
//! budget are [`crate::probe`]'s, and every read beyond this section goes
//! through it.
//!
//! (`cascade.rs` does write air, and the distinction is the whole point: it
//! cuts spill notches a few cells wide through terrace lips its containment
//! flood has already proved out against the surrounding terrain — slots for
//! water to pour through, never a room, so the cut cannot union with existing
//! geometry the way the stamped hall sheared tunnels off.)
//!
//! One dispatch runs in stages: the bounded biome GATES, the free candidate
//! gathers, the dressing's two batched crossings, then emission in claim
//! order — giants ([`giants`]), cascades ([`cascades`]), dressing
//! ([`dress`]).

mod cascades;
mod claims;
mod dress;
mod giants;

use mod_sdk::*;

use crate::content::{Content, Species};
use crate::probe::{self, Deferred, Pad};
use claims::Emitter;
use dress::Dressing;

/// Frozen positional-RNG salts. Changing one reshuffles that stream in every
/// existing world, so they are append-only in practice.
const SALT_GIANT: u64 = 0x0E58_1000_0000_0001;
const SALT_GROUND: u64 = 0x0E58_1000_0000_0002;
const SALT_CEILING: u64 = 0x0E58_1000_0000_0003;
const SALT_SPECIES: u64 = 0x0E58_1000_0000_0004;
const SALT_VINE_BLOOM: u64 = 0x0E58_1000_0000_0005;
const SALT_PATCH: u64 = 0x0E58_1000_0000_0006;

/// One in N LATTICE CELLS carries a giant-mushroom candidate. Balance data:
/// candidates are filtered again by the biome and by needing a real floor, so
/// this is much denser than the mushrooms that survive. A cavern's impact comes
/// from a stand of landmark specimens, not a forest.
const GIANT_LATTICE_ONE_IN: i32 = 5;
const VINE_MAX_LEN: i32 = 7;
/// Rows ABOVE this section that are scanned for vine roots. A curtain hangs
/// DOWN, so a root this far over our roof still drapes into us — and the
/// section that owns the root cannot write our cells, so if we do not scan for
/// it the curtain simply stops at the section plane. Cells over our roof are
/// read from the positional terrain; `GenCtx` cannot see them.
const CEILING_MARGIN: i32 = VINE_MAX_LEN - 1;
/// Terrain cells probed per margin column: every row a root may sit on, plus
/// the roof over the highest of them. The real path derives this bound from
/// the scan itself, so only the budget and invariant tests name it.
#[cfg(test)]
const PROBE_PER_MARGIN: usize = CEILING_MARGIN as usize + 1;

/// The largest horizontal reach any rolled mushroom can have. Anchors are
/// scanned this far outside the section so a cap overhanging the boundary is
/// still re-derived here. MUST be >= the worst case `Giant::reach()`.
const MAX_REACH: i32 = 14;
/// Vertical span a mushroom can occupy above its root, for the same reason.
const MAX_RISE: i32 = 26;

/// Giant mushrooms anchor on a COARSE LATTICE rather than per cell.
///
/// This is a performance contract, not a style choice. Per-cell candidacy over
/// the margin window is ~44x44x42 positional rolls for every section the
/// streamer generates — hundreds of thousands of hashes per second during
/// normal flight, for a biome that is absent from almost every section. On the
/// lattice it is a few hundred. It also spaces the mushrooms out, which reads
/// better than the clumping a per-cell roll produces.
const ANCHOR_LATTICE: i32 = 8;

/// Terrain cells probed per candidate: its whole lattice cell, plus the support
/// cell below the lowest one so every floor test is a pair inside the batch.
const PROBE_PER_CANDIDATE: usize = ANCHOR_LATTICE as usize + 1;

/// Highest world Y this feature can write anything at: a giant anchored at the
/// very top of the biome's declared depth band, at full rise.
const TOP_CONTENT_Y: i32 = crate::BIOME_TOP_Y + MAX_RISE;

/// The feature's write bounds for host-side admission: everything from the
/// world floor up to [`TOP_CONTENT_Y`]. The host skips every section above it
/// without a dispatch, so the whole gather never runs for the sky.
pub(crate) const GEN_FILTER: GenFeatureFilter = GenFeatureFilter::y_band(i32::MIN, TOP_CONTENT_Y);

/// Rows the claim set spans: this section's own sixteen, plus the margin rows
/// over its roof.
const CLAIM_ROWS: i32 = 16 + CEILING_MARGIN;

/// Giants: a candidate that reaches the section roots within a reach
/// horizontally and a rise below, and its roll can sit anywhere inside its
/// anchor lattice cell — so pad by both. Competitors beyond this decide
/// nothing on their own: they can only take a placement AWAY from a
/// candidate, and with no candidate in the biome there is none to take.
const GIANT_PAD: Pad = Pad {
    xz: MAX_REACH + ANCHOR_LATTICE,
    down: MAX_RISE + ANCHOR_LATTICE,
    up: CLAIM_ROWS - 16 + ANCHOR_LATTICE,
};

/// Dressing: a floor plant or curtain root is a cell of this section, except a
/// margin root, which sits in the rows a curtain hangs down through.
const DRESS_PAD: Pad = Pad {
    xz: 0,
    down: 0,
    up: CEILING_MARGIN,
};

/// Which passes this dispatch can be authorised for at all.
struct Gates {
    giants: bool,
    dressing: bool,
}

impl Gates {
    /// Bounded biome gates, before anything is rolled.
    ///
    /// Worldgen dispatches this feature for EVERY section in its altitude
    /// band, and a cavern is a rare event, so nearly every dispatch is a
    /// section with none of our territory in it. Rolling the candidates first
    /// and asking the biome per candidate paid the whole gather — plus a few
    /// hundred per-cell crossings — to learn that. These ask once per PASS,
    /// over exactly the reach that pass can be authorised from.
    ///
    /// Two tiers, because the two passes reach very differently and one box
    /// wide enough for both is wide enough to be true nearly everywhere: a
    /// giant is rolled on a lattice up to a full reach/rise away, while a
    /// floor plant or a curtain root is a cell of this section (plus the rows
    /// a curtain hangs through). The giant box CONTAINS the dressing box, so a
    /// `false` there settles both.
    fn ask(ours: u8, origin: [i32; 3]) -> Gates {
        let giants = probe::in_reach(ours, origin, GIANT_PAD, underground_biomes_in_box);
        let dressing =
            giants && probe::in_reach(ours, origin, DRESS_PAD, underground_biomes_in_box);
        Gates { giants, dressing }
    }
}

pub fn generate(content: &Content, ctx: &GenCtx) -> Result<Vec<GenWrite>, Deferred> {
    // The host already skips sections above the filter; a direct call (a unit
    // test) gets the same answer from the same bounds.
    if !GEN_FILTER.intersects(ctx.section_pos()[1], &[]) {
        return Ok(Vec::new());
    }
    let origin = ctx.origin_world();
    let Some(ours) = biome_id() else {
        return Ok(Vec::new());
    };
    let seed = ctx.seed();
    let gates = Gates::ask(ours, origin);

    // --- gather candidates (no host calls yet) --------------------------
    // Giants resolve through the shared, water-blind `standing_giants_over`,
    // so the emit pass and the cascades' intruder model can never drift. The
    // roll sweep here only answers "could one reach this section at all",
    // before any host call is paid for it.
    let any_giant = gates.giants && giants::could_reach(seed, origin);
    // Cascades run their own per-cell, per-worker-cached pipeline, so the
    // dressing's batches carry no cascade slots.
    let features = cascades::overlapping(seed, ours, origin)?;
    let dressing = if gates.dressing {
        Dressing::gather(content, ctx, seed)
    } else {
        Dressing::default()
    };
    if !any_giant && features.is_empty() && dressing.is_empty() {
        return Ok(Vec::new());
    }
    let Some(dressing) = dressing.resolve(origin, ours, underground_biome_at, terrain_space_at)
    else {
        return Ok(Vec::new());
    };

    // --- emission, in claim order ----------------------------------------
    // Giants FIRST, and that ordering is load-bearing: a stem's base cell is
    // the highest open cell resting on rock, which is exactly what a floor
    // flower asks for. Emitting the structure first and recording its claims
    // is what stops the flower replacing the stem.
    let mut out = Emitter::new(ctx);
    giants::emit(content, &mut out, seed, ours)?;
    cascades::emit(content, &mut out, &features);
    dressing.emit(content, &mut out, seed);
    Ok(out.into_writes())
}

/// Integer square root, for the falloff. `f64::sqrt` would be fine here, but
/// every other shape decision in this pack is integer arithmetic and mixing
/// the two invites a rounding difference between two derivations of one cell.
pub(crate) fn isqrt(n: i32) -> i32 {
    if n <= 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

/// Which species owns a spot. Derived positionally on a COARSE grid so a cavern
/// reads as stands of one colour rather than confetti — the single decision
/// that most affects how the place feels.
fn pick_species(content: &Content, seed: u32, wx: i32, wy: i32, wz: i32) -> &Species {
    // 24 put one species in an entire view, which reads monochrome. 13 lets two
    // or three stands share a sightline, so the per-channel light MIXING is
    // visible — a blue stand behind a pink one is the whole point of doing
    // coloured light properly rather than tinting.
    const STAND: i32 = 13;
    let mut rng = GenRng::positional(
        seed,
        SALT_SPECIES,
        wx.div_euclid(STAND),
        wy.div_euclid(STAND),
        wz.div_euclid(STAND),
    );
    let i = rng.next_i32(0, content.species.len() as i32 - 1) as usize;
    &content.species[i]
}

/// This pack's underground-biome id. Worldgen runs on detached instances, so
/// this is resolved per instance and cached rather than held in mod state.
fn biome_id() -> Option<u8> {
    thread_local! {
        static ID: Option<u8> = resolve_underground_biome(crate::BIOME_KEY);
    }
    ID.with(|id| *id)
}

fn is_open(ctx: &GenCtx, p: [i32; 3]) -> bool {
    matches!(ctx.block(p), Some(b) if b.0 == 0)
}

/// Is the cell `dy` from `p` ROCK? `None` when it lies outside the dispatching
/// section, where `GenCtx` knows nothing and only the positional terrain can
/// answer. Reading `None` as "not rock" is what left the section planes bare,
/// and a fluid is never rock.
fn neighbour_rock(content: &Content, ctx: &GenCtx, p: [i32; 3], dy: i32) -> Option<bool> {
    ctx.block([p[0], p[1] + dy, p[2]])
        .map(|b| b.0 != 0 && !content.is_fluid(b))
}

#[cfg(test)]
mod tests;
