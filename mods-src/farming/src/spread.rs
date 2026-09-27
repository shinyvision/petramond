use mod_sdk::*;

use crate::content::Content;
use crate::keys;
use crate::kv_counter::kv_counter_bump;

const SPREAD_CHANCE_IN: u64 = 4;
const FERTILE_TICKS: u8 = 20;
const SPREAD_RADIUS: i32 = 6;
const SPREAD_TRIES: usize = 6;

const SPREAD_KEY: &str = "farming:spread";

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
    let Some(above) = get_block([pos[0], pos[1] + 1, pos[2]]) else {
        return;
    };
    if content.spreadable.contains(&above) && rng_u64("spread").is_multiple_of(SPREAD_CHANCE_IN) {
        try_spread(content, pos, above);
    }
    let spent = kv_counter_bump(pos, SPREAD_KEY);
    if spent >= FERTILE_TICKS {
        set_block(pos, content.grass);
    } else {
        section_kv_set(pos, SPREAD_KEY, vec![spent]);
    }
}

struct Candidate {
    soil: [i32; 3],
    head: [i32; 3],
}

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
        let rooted =
            matches!(pair[0], Some(b) if b == content.grass || b == content.grass_fertilized);
        if !rooted || pair[1] != Some(BlockId::AIR) {
            continue;
        }
        set_block(c.head, plant);
        emitter_burst(
            keys::FERTILIZE_BURST,
            [
                c.head[0] as f64 + 0.5,
                c.head[1] as f64 + 0.3,
                c.head[2] as f64 + 0.5,
            ],
            1.0,
        );
        return;
    }
}
