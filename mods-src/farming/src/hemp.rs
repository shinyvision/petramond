use mod_sdk::*;

use crate::content::Content;
use crate::keys;

const SEED_DROP_IN: u64 = 10;
const SEED_COUNT: (u64, u64) = (1, 2);

pub fn on_block_broken(content: &Content, pos: [i32; 3], block: BlockId, natural: bool) {
    if natural || block != content.hemp_wild {
        return;
    }
    if !rng_u64("hemp_seeds").is_multiple_of(SEED_DROP_IN) {
        return;
    }
    let (lo, hi) = SEED_COUNT;
    let count = (lo + rng_u64("hemp_seed_count") % (hi - lo + 1)) as u8;
    spawn_item(
        keys::HEMP_SEEDS,
        count,
        [
            pos[0] as f64 + 0.5,
            pos[1] as f64 + 0.3,
            pos[2] as f64 + 0.5,
        ],
    );
}
