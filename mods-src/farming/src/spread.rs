//! Fertilized grass: the spreading-lawn block fertilizer leaves behind.
//!
//! How a grass block BECOMES fertilized is [`crate::fertilize`]'s business
//! (the fertilizer target table); this module owns what the block then does.
//! On each of its RANDOM ticks, if soil-rooted vegetation (a flower, short
//! grass, a fern, a mushroom — anything carrying `petramond:roots_in_soil`
//! except saplings) stands on it, it rolls a spread: a copy of that plant
//! takes root on a nearby grass block. After [`FERTILE_TICKS`] random ticks
//! the fertility is spent and the block relaxes back to plain grass (the
//! block write clears its cell KV). Fertilized grass with nothing rooted on
//! it spreads nothing — the countdown still runs.

use mod_sdk::*;

use crate::content::Content;
use crate::kv_counter::kv_counter_bump;

/// One-in-N spread roll per random tick with vegetation on top (4 → 25%).
const SPREAD_CHANCE_IN: u64 = 4;
/// Random ticks of fertility before the block relaxes back to plain grass.
const FERTILE_TICKS: u8 = 20;
/// Square horizontal radius a spread may reach (|dx| ≤ 6, |dz| ≤ 6, one step
/// of terrain up or down).
const SPREAD_RADIUS: i32 = 6;
/// Candidate cells sampled per successful roll; the first valid one wins.
/// Missing every try is fine — the next roll re-samples fresh candidates.
const SPREAD_TRIES: usize = 6;

/// Cell-KV key for the spent-random-ticks counter on fertilized grass.
const SPREAD_KEY: &str = "farming:spread";

/// Fertilized-grass block hooks: only random ticks do work — the spread roll
/// and the spent-fertility countdown.
pub fn on_hook(content: &Content, kind: BlockHookKind, pos: [i32; 3]) {
    match kind {
        BlockHookKind::RandomTick => random_tick(content, pos),
        BlockHookKind::NeighborUpdate | BlockHookKind::ScheduledTick => {}
    }
}

fn random_tick(content: &Content, pos: [i32; 3]) {
    let Some(current) = get_block(pos) else {
        return;
    };
    if current != content.grass_fertilized {
        return;
    }
    // An unreadable (streaming) cell above neither spreads nor counts — no
    // state movement on missing information, the farmland-idle rule.
    let Some(above) = get_block([pos[0], pos[1] + 1, pos[2]]) else {
        return;
    };
    if content.spreadable.contains(&above) && rng_u64("spread") % SPREAD_CHANCE_IN == 0 {
        try_spread(content, pos, above);
    }
    let spent = kv_counter_bump(pos, SPREAD_KEY);
    if spent >= FERTILE_TICKS {
        // Spent: back to plain grass — the block write clears the cell KV.
        set_block(pos, content.grass);
    } else {
        section_kv_set(pos, SPREAD_KEY, vec![spent]);
    }
}

/// A sampled spread target: the soil cell a plant could root ON and the
/// head cell (directly above) the plant would occupy.
struct Candidate {
    soil: [i32; 3],
    head: [i32; 3],
}

/// Plant one copy of `plant` on a nearby grass block: a handful of jittered
/// candidates, batch-read (target soil + its headroom), first valid wins.
fn try_spread(content: &Content, pos: [i32; 3], plant: BlockId) {
    let side = (SPREAD_RADIUS * 2 + 1) as u64;
    let mut candidates = Vec::with_capacity(SPREAD_TRIES);
    for _ in 0..SPREAD_TRIES {
        let dx = (rng_u64("spread") % side) as i32 - SPREAD_RADIUS;
        let dz = (rng_u64("spread") % side) as i32 - SPREAD_RADIUS;
        let dy = (rng_u64("spread") % 3) as i32 - 1;
        if dx == 0 && dz == 0 {
            continue;
        }
        let soil = [pos[0] + dx, pos[1] + dy, pos[2] + dz];
        candidates.push(Candidate {
            soil,
            head: [soil[0], soil[1] + 1, soil[2]],
        });
    }
    let cells = candidates.iter().flat_map(|c| [c.soil, c.head]).collect();
    let got = get_blocks(cells);
    for (pair, c) in got.chunks_exact(2).zip(&candidates) {
        let rooted = matches!(pair[0], Some(b) if b == content.grass || b == content.grass_fertilized);
        if !rooted || pair[1] != Some(BlockId::AIR) {
            continue;
        }
        set_block(c.head, plant);
        emitter_burst(
            "farming:fertilize_burst",
            [
                c.head[0] as f32 + 0.5,
                c.head[1] as f32 + 0.3,
                c.head[2] as f32 + 0.5,
            ],
            1.0,
        );
        return;
    }
}
