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

const SALT_GIANT: u64 = 0x0E58_1000_0000_0001;
const SALT_GROUND: u64 = 0x0E58_1000_0000_0002;
const SALT_CEILING: u64 = 0x0E58_1000_0000_0003;
const SALT_SPECIES: u64 = 0x0E58_1000_0000_0004;
const SALT_VINE_BLOOM: u64 = 0x0E58_1000_0000_0005;
const SALT_PATCH: u64 = 0x0E58_1000_0000_0006;

const GIANT_LATTICE_ONE_IN: i32 = 5;
const VINE_MAX_LEN: i32 = 7;
/// Rows ABOVE this section that are scanned for vine roots. A curtain hangs
/// DOWN, so a root this far over our roof still drapes into us — and the
/// section that owns the root cannot write our cells, so if we do not scan for
/// it the curtain simply stops at the section plane. Cells over our roof are
/// read from the positional terrain; `GenCtx` cannot see them.
const CEILING_MARGIN: i32 = VINE_MAX_LEN - 1;
#[cfg(test)]
const PROBE_PER_MARGIN: usize = CEILING_MARGIN as usize + 1;

const MAX_REACH: i32 = 14;
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

const PROBE_PER_CANDIDATE: usize = ANCHOR_LATTICE as usize + 1;

const TOP_CONTENT_Y: i32 = crate::BIOME_TOP_Y + MAX_RISE;

pub(crate) const GEN_FILTER: GenFeatureFilter = GenFeatureFilter::y_band(i32::MIN, TOP_CONTENT_Y);

const CLAIM_ROWS: i32 = 16 + CEILING_MARGIN;

const GIANT_PAD: Pad = Pad {
    xz: MAX_REACH + ANCHOR_LATTICE,
    down: MAX_RISE + ANCHOR_LATTICE,
    up: CLAIM_ROWS - 16 + ANCHOR_LATTICE,
};

const DRESS_PAD: Pad = Pad {
    xz: 0,
    down: 0,
    up: CEILING_MARGIN,
};

struct Gates {
    giants: bool,
    dressing: bool,
}

impl Gates {
    fn ask(ours: u8, origin: [i32; 3]) -> Gates {
        let giants = probe::in_reach(ours, origin, GIANT_PAD, underground_biomes_in_box);
        let dressing =
            giants && probe::in_reach(ours, origin, DRESS_PAD, underground_biomes_in_box);
        Gates { giants, dressing }
    }
}

pub fn generate(content: &Content, ctx: &GenCtx) -> Result<Vec<GenWrite>, Deferred> {
    if !GEN_FILTER.intersects(ctx.section_pos()[1], &[]) {
        return Ok(Vec::new());
    }
    let origin = ctx.origin_world();
    let Some(ours) = biome_id() else {
        return Ok(Vec::new());
    };
    let seed = ctx.seed();
    let gates = Gates::ask(ours, origin);

    let any_giant = gates.giants && giants::could_reach(seed, origin);
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

    let mut out = Emitter::new(ctx);
    giants::emit(content, &mut out, seed, ours)?;
    cascades::emit(content, &mut out, &features);
    dressing.emit(content, &mut out, seed);
    Ok(out.into_writes())
}

fn pick_species(content: &Content, seed: u32, wx: i32, wy: i32, wz: i32) -> &Species {
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

fn biome_id() -> Option<u8> {
    thread_local! {
        static ID: Option<u8> = resolve_underground_biome(crate::BIOME_KEY);
    }
    ID.with(|id| *id)
}

fn is_open(ctx: &GenCtx, p: [i32; 3]) -> bool {
    matches!(ctx.block(p), Some(b) if b.0 == 0)
}

fn neighbour_rock(content: &Content, ctx: &GenCtx, p: [i32; 3], dy: i32) -> Option<bool> {
    ctx.block([p[0], p[1] + dy, p[2]])
        .map(|b| b.0 != 0 && !content.is_fluid(b))
}

#[cfg(test)]
mod tests;
