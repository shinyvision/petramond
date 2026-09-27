use mod_sdk::*;

use crate::content::Content;
use crate::keys;

const SEED_DROP_IN: u64 = 100;

pub fn on_block_broken(content: &Content, pos: [i32; 3], block: BlockId, natural: bool) {
    if natural {
        return;
    }
    if !content.seed_cover.contains(&block) {
        return;
    }
    if !rng_u64("forage_seeds").is_multiple_of(SEED_DROP_IN) {
        return;
    }
    spawn_item(
        keys::WHEAT_SEEDS,
        1,
        [
            pos[0] as f64 + 0.5,
            pos[1] as f64 + 0.3,
            pos[2] as f64 + 0.5,
        ],
    );
}
