//! A feature's declared underground-biome gate, answered before its dispatch is encoded.
//!
//! The exact question is per section; its answer is memoized by the terrain query's own box
//! memo. Because most sections lie nowhere near the feature's biome, a coarser verdict per
//! block of 2×2×2 sections (the block's box padded like a section's) is kept here and settles
//! them without a query each: a box containing the section's box can only admit more.

use std::sync::RwLock;

use mod_api::UndergroundGate;
use rustc_hash::FxHashMap;

const BLOCK: i32 = 2;
const MAX_VERDICTS: usize = 1 << 16;

#[derive(Default)]
pub(super) struct GateVerdicts {
    blocks: RwLock<FxHashMap<[i32; 3], bool>>,
}

impl GateVerdicts {
    pub(super) fn admits(&self, gate: &UndergroundGate, seed: u32, section: [i32; 3]) -> bool {
        if gate.biomes.is_empty() {
            return false;
        }
        let block = section.map(|v| v.div_euclid(BLOCK));
        let known = self
            .blocks
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&block)
            .copied();
        let maybe = match known {
            Some(v) => v,
            None => {
                let (lo, _) = gate.around(block.map(|v| v * BLOCK));
                let (_, hi) = gate.around(block.map(|v| v * BLOCK + BLOCK - 1));
                let v = petramond_worldgen::underground_box_admits(seed, lo, hi, &gate.biomes.0);
                let mut blocks = self.blocks.write().unwrap_or_else(|e| e.into_inner());
                if blocks.len() >= MAX_VERDICTS {
                    blocks.clear();
                }
                blocks.insert(block, v);
                v
            }
        };
        if !maybe {
            return false;
        }
        let (lo, hi) = gate.around(section);
        petramond_worldgen::underground_box_admits(seed, lo, hi, &gate.biomes.0)
    }
}
