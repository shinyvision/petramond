//! Foraging scraps: a player breaking living ground cover (short grass,
//! fern) rarely shakes loose a wheat seed — the fallback seed source for a
//! spawn region with no wild wheat patch in walking range.
//!
//! Player breaks only (`natural: false`): world-sim destruction (water
//! washing a meadow away) must not be a seed faucet, and `harvested` keeps
//! the roll off drop-less breaks. The chance is balance data.

use mod_sdk::*;

use crate::content::Content;

/// One-in-N chance per broken cover block (100 → 1%).
const SEED_DROP_IN: u64 = 100;

pub fn on_block_broken(
    content: &Content,
    pos: [i32; 3],
    block: BlockId,
    harvested: bool,
    natural: bool,
) {
    if natural || !harvested {
        return;
    }
    if !content.seed_cover.contains(&block) {
        return;
    }
    if rng_u64("forage_seeds") % SEED_DROP_IN != 0 {
        return;
    }
    spawn_item(
        "farming:wheat_seeds",
        1,
        [
            pos[0] as f32 + 0.5,
            pos[1] as f32 + 0.3,
            pos[2] as f32 + 0.5,
        ],
    );
}
