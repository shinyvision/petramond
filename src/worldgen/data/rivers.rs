//! River bed/bank material tables — biome→block authoring surface.
//!
//! Mirrors the `data::features` convention: per-biome material choices are pure
//! data knobs, edited here in one place. Each function takes the river system's
//! shared OpenSimplex sampler (`material` for bed/bank deposits, `bank` for
//! steepness) so the noise draws stay byte-identical to the inline carve — the
//! arithmetic is moved verbatim, only relocated out of `RiverSystem`.

use noise::{NoiseFn, OpenSimplex};

use crate::biome::Biome;
use crate::block::Block;
use crate::mathh::smoothstep;

pub fn bed_block(material: &OpenSimplex, wx: i32, wz: i32, biome: Biome) -> Block {
    let material_noise = material.get([wx as f64 * 0.0045, wz as f64 * 0.0045]) as f32;
    let sand_bias = match biome {
        Biome::Ocean | Biome::DeepOcean | Biome::Beach | Biome::Desert => 0.72,
        Biome::Badlands | Biome::Savanna => 0.48,
        Biome::Swamp | Biome::Wetland => 0.12,
        Biome::Mountains | Biome::SnowyPeaks | Biome::StonyPeaks | Biome::WindsweptHills => -0.12,
        _ => -0.26,
    };
    let gravel_bias = match biome {
        Biome::Mountains
        | Biome::SnowyPeaks
        | Biome::StonyPeaks
        | Biome::WindsweptHills
        | Biome::Foothills => 0.18,
        _ => -0.20,
    };
    if sand_bias + material_noise * 0.42 > 0.34 {
        Block::Sand
    } else if gravel_bias + material_noise * 0.36 > 0.18 {
        Block::Gravel
    } else if material_noise < -0.34 {
        Block::CoarseDirt
    } else {
        Block::Dirt
    }
}

pub fn bank_block(
    material: &OpenSimplex,
    wx: i32,
    wz: i32,
    biome: Biome,
    influence: f32,
    width: f32,
) -> Option<Block> {
    let deposit_noise =
        material.get([wx as f64 * 0.0065 + 211.0, wz as f64 * 0.0065 - 109.0]) as f32 * 0.5 + 0.5;
    // §8 retune: influence now plateaus near 1 across the channel+floodplain,
    // so the gate knee is pushed out to keep deposits to the inner banks.
    let zone = smoothstep(0.45, 0.9, influence) * smoothstep(4.0, 15.0, width);
    let chance = match biome {
        Biome::Ocean | Biome::DeepOcean | Biome::Beach | Biome::Desert => 0.88,
        Biome::Badlands => 0.78,
        Biome::Savanna => 0.46,
        Biome::Mountains
        | Biome::SnowyPeaks
        | Biome::StonyPeaks
        | Biome::WindsweptHills
        | Biome::Foothills
        | Biome::SnowySlopes => 0.38,
        Biome::Swamp | Biome::Wetland => 0.14,
        Biome::Plains
        | Biome::Meadow
        | Biome::Forest
        | Biome::BirchForest
        | Biome::DarkForest
        | Biome::Jungle
        | Biome::CherryGrove
        | Biome::Taiga
        | Biome::OldGrowthTaiga => 0.18,
        _ => 0.24,
    } * zone;
    if deposit_noise > chance {
        return None;
    }

    Some(match biome {
        Biome::Badlands => Block::RedSand,
        Biome::Ocean | Biome::DeepOcean | Biome::Beach | Biome::Desert | Biome::Savanna => {
            Block::Sand
        }
        Biome::Mountains
        | Biome::SnowyPeaks
        | Biome::StonyPeaks
        | Biome::WindsweptHills
        | Biome::Foothills
        | Biome::SnowySlopes => Block::Gravel,
        _ => Block::Gravel,
    })
}

pub fn bank_steepness(bank: &OpenSimplex, wx: i32, wz: i32, biome: Biome, relief: f32) -> f32 {
    let biome_bias = match biome {
        Biome::Mountains
        | Biome::SnowyPeaks
        | Biome::StonyPeaks
        | Biome::WindsweptHills
        | Biome::SnowySlopes => 0.82,
        Biome::Foothills | Biome::Grove | Biome::OldGrowthTaiga => 0.58,
        Biome::Badlands | Biome::Savanna => 0.45,
        Biome::Forest | Biome::BirchForest | Biome::DarkForest | Biome::Jungle => 0.34,
        Biome::Plains | Biome::Meadow | Biome::CherryGrove => 0.24,
        _ => 0.30,
    };
    let relief_bias = smoothstep(10.0, 76.0, relief);
    let noise = bank.get([wx as f64 * 0.006, wz as f64 * 0.006]) as f32 * 0.5 + 0.5;
    (biome_bias * 0.56 + relief_bias * 0.30 + noise * 0.14).clamp(0.0, 1.0)
}
