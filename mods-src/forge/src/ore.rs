//! Petramond ore: the namesake gem, VERY deep down.
//!
//! Small veins in the band below the cave floor (`CAVE_MIN_Y` is −48, so
//! most of the band is reachable only by deliberate digging), rarer than
//! diamond by an order of magnitude and weighted toward the world floor.
//! The gem's one use is carving augment sockets open at the anvil
//! (`anvil.rs`), so its rarity IS the socket economy — retune with
//! `orecensus`, never by arithmetic.
//!
//! SEAM CONTRACT (the engine scatter's own rule, worn mod-side): every vein
//! derives from its OWN positional RNG, keyed by the CHUNK COLUMN it is
//! anchored in and its index — so every section the vein touches re-derives
//! the identical vein and clips it to the cells it owns. Acceptance reads
//! nothing but position; `ctx.block` is only the per-cell stone clip, which
//! is exactly the question it may answer.

use mod_sdk::*;

use crate::keys;

const SALT_VEIN: u64 = 0xF012_04E0_0000_0001;

const Y_MIN: i32 = -64;
const Y_MAX: i32 = -40;

pub(crate) const GEN_FILTER: GenFeatureFilter = GenFeatureFilter::y_band(Y_MIN, Y_MAX + 1);

const VEINS_PER_COLUMN: i32 = 11;

const FLOOR_CHANCE: f32 = 1.0;

const SIZE_MIN: i32 = 1;
const SIZE_MAX: i32 = 2;

const CLUSTER: [[i32; 3]; 6] = [
    [1, 0, 0],
    [-1, 0, 0],
    [0, 0, 1],
    [0, 0, -1],
    [0, 1, 0],
    [0, -1, 0],
];

#[derive(Default)]
pub struct Ore {
    ore: Option<BlockId>,
    stone: Option<BlockId>,
}

impl Ore {
    pub fn init(&mut self) {
        self.ore = resolve_block_logged(keys::PETRAMOND_ORE);
        self.stone = resolve_block_logged(keys::STONE);
    }

    pub fn generate(&self, ctx: &GenCtx) -> Vec<GenWrite> {
        let (Some(ore), Some(stone)) = (self.ore, self.stone) else {
            return Vec::new();
        };
        if !GEN_FILTER.intersects(ctx.section_pos()[1], &[]) {
            return Vec::new();
        }
        let origin = ctx.origin_world();
        let seed = ctx.seed();
        let (cx, cz) = (origin[0] >> 4, origin[2] >> 4);

        let mut writes = Vec::new();
        for ncx in (cx - 1)..=(cx + 1) {
            for ncz in (cz - 1)..=(cz + 1) {
                for i in 0..VEINS_PER_COLUMN {
                    let mut rng = GenRng::positional(seed, SALT_VEIN, ncx, i, ncz);
                    let ox = (ncx << 4) + rng.next_i32(0, 15);
                    let oz = (ncz << 4) + rng.next_i32(0, 15);
                    let oy = rng.next_i32(Y_MIN, Y_MAX);
                    let t = (Y_MAX - oy) as f32 / (Y_MAX - Y_MIN) as f32;
                    if !rng.chance(FLOOR_CHANCE * t * t) {
                        continue;
                    }
                    let size = rng.next_i32(SIZE_MIN, SIZE_MAX);
                    let mut cells = vec![[ox, oy, oz]];
                    for _ in 1..size {
                        let off = CLUSTER[rng.next_i32(0, CLUSTER.len() as i32 - 1) as usize];
                        cells.push([ox + off[0], oy + off[1], oz + off[2]]);
                    }
                    for pos in cells {
                        if ctx.block(pos) == Some(stone) {
                            writes.push((pos, ore));
                        }
                    }
                }
            }
        }
        writes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_at(section: [i32; 3], stone: BlockId) -> GenCtx {
        GenCtx::for_test(
            section,
            0x5EED,
            vec![stone.0; 16 * 16 * 16],
            vec![0; 256],
            vec![0; 256],
            0,
        )
    }

    #[test]
    fn adjacent_sections_agree_on_the_veins_and_emit_only_their_own_cells() {
        let stone = BlockId(7);
        let ore = Ore {
            ore: Some(BlockId(200)),
            stone: Some(stone),
        };
        let cy = -4;
        let mut all = Vec::new();
        for scx in 0..4 {
            let writes = ore.generate(&ctx_at([scx, cy, 0], stone));
            for (pos, block) in writes {
                assert_eq!(block, BlockId(200));
                assert!(
                    (scx * 16..scx * 16 + 16).contains(&pos[0])
                        && (cy * 16..cy * 16 + 16).contains(&pos[1])
                        && (0..16).contains(&pos[2]),
                    "a section may only emit cells it owns: {pos:?} from section x {scx}"
                );
                all.push(pos);
            }
        }
        let mut dedup = all.clone();
        dedup.sort();
        dedup.dedup();
        assert_eq!(dedup.len(), all.len(), "no cell is written twice");
        assert!(
            !all.is_empty(),
            "the deep band generates SOME ore over 4 sections"
        );
    }

    #[test]
    fn the_ore_stays_in_its_deep_band() {
        let stone = BlockId(7);
        let ore = Ore {
            ore: Some(BlockId(200)),
            stone: Some(stone),
        };
        for scx in 0..8 {
            for (pos, _) in ore.generate(&ctx_at([scx, -4, 0], stone)) {
                assert!(pos[1] >= Y_MIN && pos[1] <= Y_MAX + 1, "band: {pos:?}");
            }
        }
        assert!(ore.generate(&ctx_at([0, 0, 0], stone)).is_empty());
        assert!(ore.generate(&ctx_at([0, 4, 0], stone)).is_empty());
    }
}
