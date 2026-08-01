//! Hemp seeds: what breaking a WILD hemp stand sometimes shakes loose.
//!
//! PLANTING is not here and needs no code — `farming:hemp_seeds` is a pack
//! item, so its row's `block` link plants `farming:hemp_0` through ordinary
//! placement, and `crops::on_place_pre` already gates that to farmland with the
//! darkness veto, exactly as it does for wheat.
//!
//! The core `petramond:hemp` fibre is a MATERIAL and places nothing — it lost
//! its `block` link when replanting it turned out to be a seed mint (see
//! WIKI/progression.md). Seeds farm; fibre is the product.

use mod_sdk::*;

use crate::content::Content;

/// One-in-N chance that breaking a hemp plant shakes seeds loose (10 → 10%),
/// and the count it then gives. Balance data.
const SEED_DROP_IN: u64 = 10;
const SEED_COUNT: (u64, u64) = (1, 2);

/// Breaking the ENGINE's WILD hemp stand rarely shakes seeds loose. This is the
/// only route from a wild stand to a farmed one, so it is what makes hemp
/// renewable at all.
///
/// ONLY the wild stand, because only the wild stand needs code: it is a core
/// row this pack cannot give drops to. Every cultivated stage declares its own
/// drops in `blocks.json`, where a `chance` field expresses the same idea
/// directly — adding a roll here as well would double them.
///
/// Player breaks only, the `forage` rule: water washing a hemp bed away must
/// not be a seed faucet. And the wild stand cannot be REPLANTED (its item has
/// no `block` link, like every other wild crop) — without that, this roll would
/// be a place-break-repeat mint rather than a find.
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
        "farming:hemp_seeds",
        count,
        [
            pos[0] as f32 + 0.5,
            pos[1] as f32 + 0.3,
            pos[2] as f32 + 0.5,
        ],
    );
}
