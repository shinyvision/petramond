//! Explicit river network and column carver.
//!
//! This replaces the classic biome-layer river overlay for active generation.
//! Rivers are generated as deterministic path objects first, then columns query
//! distance to those paths for a single channel/bank cross-section.

use noise::{Fbm, MultiFractal, NoiseFn, OpenSimplex};

use crate::biome::Biome;
use crate::block::Block;
use crate::chunk::SEA_LEVEL;
use crate::mathh::smoothstep;

use super::classic::world::{map_biome, RegionCells};
use super::rng::FeatureRng;

const SOURCE_SALT: u64 = 0x0000_5249_5645_5253;
const CELL_BLOCKS: i32 = 640;
const PATH_STEPS: usize = 38;
const STEP_BLOCKS: f32 = 30.0;
const PATH_REACH: i32 = 1_260;
const MAX_QUERY_RADIUS: f32 = 96.0;
const MIN_WET_WIDTH: f32 = 1.6;

const SALT_SOURCE: u32 = 0x5210_0001;
const SALT_POTENTIAL: u32 = 0x5210_0002;
const SALT_BEND: u32 = 0x5210_0003;
const SALT_WIDTH: u32 = 0x5210_0004;
const SALT_DEPTH: u32 = 0x5210_0005;
const SALT_MATERIAL: u32 = 0x5210_0006;
const SALT_BANK: u32 = 0x5210_0007;

/// River effect at one world column after querying the explicit path network.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct RiverColumn {
    /// 0 outside banks, 1 near the channel.
    pub influence: f32,
    /// 0 outside the wet channel, 1 at the centerline.
    pub channel: f32,
    /// Distance in blocks to the chosen centerline segment.
    pub distance: f32,
    /// Full wet-channel width in blocks at the nearest segment.
    pub width: f32,
    /// River depth in blocks at the nearest segment.
    pub depth: f32,
    /// Top solid river-bed y after carving.
    pub bed_y: i32,
    /// Water surface y for this river column.
    pub water_y: i32,
    /// Block used for exposed river banks and bed, unless an existing water-body
    /// floor should preserve its own material.
    pub bed_block: Block,
    /// Optional exposed bank deposit. `None` means the smoothed bank keeps its
    /// surrounding biome surface, which is common through grass-dominant biomes.
    pub bank_block: Option<Block>,
    /// Existing water-body floors keep their original biome surface rule.
    pub preserve_bed: bool,
}

impl RiverColumn {
    #[inline]
    pub fn active(self) -> bool {
        self.influence > 0.01
    }

    #[inline]
    pub fn wet(self) -> bool {
        self.width >= MIN_WET_WIDTH && self.channel > 0.05
    }
}

impl Default for RiverColumn {
    fn default() -> Self {
        Self {
            influence: 0.0,
            channel: 0.0,
            distance: f32::INFINITY,
            width: 0.0,
            depth: 0.0,
            bed_y: SEA_LEVEL - 4,
            water_y: SEA_LEVEL,
            bed_block: Block::Dirt,
            bank_block: None,
            preserve_bed: false,
        }
    }
}

pub struct RiverSystem {
    seed: u32,
    source: Fbm<OpenSimplex>,
    potential: Fbm<OpenSimplex>,
    bend: OpenSimplex,
    width: OpenSimplex,
    depth: OpenSimplex,
    material: OpenSimplex,
    bank: OpenSimplex,
    tilt_x: f32,
    tilt_z: f32,
}

impl RiverSystem {
    pub fn new(seed: u32) -> Self {
        let mut rng = FeatureRng::positional(seed, SOURCE_SALT, 0, 0, 0);
        let angle = rng.next_f32() * std::f32::consts::TAU;
        Self {
            seed,
            source: Fbm::<OpenSimplex>::new(seed.wrapping_add(SALT_SOURCE))
                .set_octaves(2)
                .set_frequency(0.0017),
            potential: Fbm::<OpenSimplex>::new(seed.wrapping_add(SALT_POTENTIAL))
                .set_octaves(3)
                .set_frequency(0.0011),
            bend: OpenSimplex::new(seed.wrapping_add(SALT_BEND)),
            width: OpenSimplex::new(seed.wrapping_add(SALT_WIDTH)),
            depth: OpenSimplex::new(seed.wrapping_add(SALT_DEPTH)),
            material: OpenSimplex::new(seed.wrapping_add(SALT_MATERIAL)),
            bank: OpenSimplex::new(seed.wrapping_add(SALT_BANK)),
            tilt_x: angle.cos(),
            tilt_z: angle.sin(),
        }
    }

    /// Apply river carving in-place to a base land region.
    pub fn apply(&self, region: &mut RegionCells) {
        let paths = self.paths_for_bounds(
            region.x0,
            region.z0,
            region.x0 + region.w as i32,
            region.z0 + region.h as i32,
        );
        region.rivers.fill(RiverColumn::default());
        if paths.is_empty() {
            return;
        }

        for z in 0..region.h {
            for x in 0..region.w {
                let i = z * region.w + x;
                let wx = region.x0 + x as i32;
                let wz = region.z0 + z as i32;
                let base_surf = region.surf[i];
                let biome = map_biome(region.biome_ids[i]);
                let Some((river, carved_surf)) =
                    self.carve_column(wx, wz, base_surf, biome, &paths)
                else {
                    continue;
                };

                region.rivers[i] = river;
                region.surf[i] = carved_surf;
                if river.wet() && !river.preserve_bed {
                    region.biome_ids[i] = 7; // classic river id, mapped by `map_biome`.
                }
            }
        }
    }

    fn carve_column(
        &self,
        wx: i32,
        wz: i32,
        base_surf: i32,
        biome: Biome,
        paths: &[RiverPath],
    ) -> Option<(RiverColumn, i32)> {
        if matches!(biome, Biome::DeepOcean | Biome::MushroomFields) {
            return None;
        }

        let hit = nearest_hit(wx as f32 + 0.5, wz as f32 + 0.5, paths)?;
        let channel_half = hit.width * 0.5;
        let wet_edge = channel_half * 0.95;
        let relief = (base_surf - SEA_LEVEL).max(0) as f32;
        let preserve_bed = base_surf <= SEA_LEVEL;
        let steepness = self.bank_steepness(wx, wz, biome, relief);
        let size = smoothstep(0.0, 8.0, hit.width);
        let shelf_width = self.shelf_width(wx, wz, steepness) * size;
        let bank_extra = self.bank_extra(hit.width, relief, steepness, preserve_bed) * size;
        let shelf_outer = wet_edge + shelf_width;
        let influence_radius = shelf_outer + bank_extra;
        if influence_radius < 0.5 {
            return None;
        }
        if hit.distance >= influence_radius {
            return None;
        }

        let bank_t = smoothstep(shelf_outer, influence_radius, hit.distance);
        let influence = 1.0 - bank_t;
        if influence <= 0.01 {
            return None;
        }

        let channel = if channel_half > 0.01 {
            (1.0 - hit.distance / channel_half).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let bed_y = (SEA_LEVEL as f32 - hit.depth).round().max(3.0) as i32;

        let target = if hit.distance <= wet_edge && wet_edge > 0.05 {
            let t = (hit.distance / wet_edge).clamp(0.0, 1.0);
            bed_y as f32 + smoothstep(0.0, 1.0, t) * ((SEA_LEVEL - 1 - bed_y).max(0) as f32)
        } else if hit.distance <= shelf_outer {
            SEA_LEVEL as f32
        } else {
            let smoothed = SEA_LEVEL as f32
                + (base_surf as f32 - SEA_LEVEL as f32) * bank_t.powf(1.08 + steepness * 0.72);
            let varied = smoothed + self.bank_height_variation(wx, wz, relief, steepness, bank_t);
            if base_surf >= SEA_LEVEL {
                varied.clamp(SEA_LEVEL as f32, base_surf as f32)
            } else {
                varied.min(base_surf as f32)
            }
        };
        let carved_surf = (target.round() as i32).min(base_surf).max(3);
        if carved_surf >= base_surf && influence < 0.08 {
            return None;
        }

        Some((
            RiverColumn {
                influence,
                channel,
                distance: hit.distance,
                width: hit.width,
                depth: hit.depth,
                bed_y,
                water_y: SEA_LEVEL,
                bed_block: self.bed_block(wx, wz, biome),
                bank_block: self.bank_block(wx, wz, biome, influence, hit.width),
                preserve_bed,
            },
            carved_surf,
        ))
    }

    fn paths_for_bounds(&self, x0: i32, z0: i32, x1: i32, z1: i32) -> Vec<RiverPath> {
        let cx0 = (x0 - PATH_REACH).div_euclid(CELL_BLOCKS);
        let cz0 = (z0 - PATH_REACH).div_euclid(CELL_BLOCKS);
        let cx1 = (x1 + PATH_REACH).div_euclid(CELL_BLOCKS);
        let cz1 = (z1 + PATH_REACH).div_euclid(CELL_BLOCKS);
        let mut paths = Vec::new();
        for cz in cz0..=cz1 {
            for cx in cx0..=cx1 {
                if let Some(path) = self.path_from_cell(cx, cz) {
                    if path.intersects(x0 as f32, z0 as f32, x1 as f32, z1 as f32) {
                        paths.push(path);
                    }
                }
            }
        }
        paths
    }

    fn path_from_cell(&self, cx: i32, cz: i32) -> Option<RiverPath> {
        let ox = cx * CELL_BLOCKS;
        let oz = cz * CELL_BLOCKS;
        let center_x = ox + CELL_BLOCKS / 2;
        let center_z = oz + CELL_BLOCKS / 2;
        let mut rng = FeatureRng::positional(self.seed, SOURCE_SALT, cx, 0, cz);
        let score =
            self.source.get([center_x as f64, center_z as f64]) as f32 + rng.next_f32() * 0.35;
        if score < -0.12 {
            return None;
        }

        let mut x = ox as f32 + CELL_BLOCKS as f32 * (0.20 + 0.60 * rng.next_f32());
        let mut z = oz as f32 + CELL_BLOCKS as f32 * (0.20 + 0.60 * rng.next_f32());
        let mut dir = unit_from_angle(rng.next_f32() * std::f32::consts::TAU);
        let mut points = Vec::with_capacity(PATH_STEPS + 1);
        let mut min_x = x;
        let mut min_z = z;
        let mut max_x = x;
        let mut max_z = z;

        for step in 0..=PATH_STEPS {
            let downstream = step as f32 / PATH_STEPS as f32;
            let w = self.channel_width(x, z, downstream);
            let depth = self.channel_depth(x, z, w, downstream);
            points.push(RiverPoint {
                x,
                z,
                width: w,
                depth,
            });
            min_x = min_x.min(x);
            min_z = min_z.min(z);
            max_x = max_x.max(x);
            max_z = max_z.max(z);
            dir = self.flow_dir(x, z, dir);
            x += dir.0 * STEP_BLOCKS;
            z += dir.1 * STEP_BLOCKS;
        }

        Some(RiverPath {
            points,
            min_x: min_x - MAX_QUERY_RADIUS,
            min_z: min_z - MAX_QUERY_RADIUS,
            max_x: max_x + MAX_QUERY_RADIUS,
            max_z: max_z + MAX_QUERY_RADIUS,
        })
    }

    fn channel_width(&self, x: f32, z: f32, downstream: f32) -> f32 {
        let wobble = self.width.get([x as f64 * 0.010, z as f64 * 0.010]) as f32;
        let base = 7.5 + downstream * 22.0 + wobble * 6.0;
        (base * end_taper(downstream)).clamp(0.0, 40.0)
    }

    fn channel_depth(&self, x: f32, z: f32, width: f32, downstream: f32) -> f32 {
        let wobble = self.depth.get([x as f64 * 0.007, z as f64 * 0.007]) as f32;
        let base = 2.4 + width * 0.18 + downstream * 3.3 + wobble * 1.8;
        (base * end_taper(downstream).sqrt()).clamp(0.0, 11.0)
    }

    fn shelf_width(&self, wx: i32, wz: i32, steepness: f32) -> f32 {
        let noise = self
            .bank
            .get([wx as f64 * 0.011 + 91.0, wz as f64 * 0.011 - 37.0]) as f32
            * 0.5
            + 0.5;
        (1.45 + (1.0 - steepness) * 0.75 + noise * 0.45).clamp(1.6, 2.8)
    }

    fn bank_steepness(&self, wx: i32, wz: i32, biome: Biome, relief: f32) -> f32 {
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
        let noise = self.bank.get([wx as f64 * 0.006, wz as f64 * 0.006]) as f32 * 0.5 + 0.5;
        (biome_bias * 0.56 + relief_bias * 0.30 + noise * 0.14).clamp(0.0, 1.0)
    }

    fn bank_extra(&self, width: f32, relief: f32, steepness: f32, preserve_bed: bool) -> f32 {
        if preserve_bed {
            return (width * 0.45 + 7.0).clamp(8.0, 22.0);
        }
        let relief_scale = 0.52 - steepness * 0.34;
        (6.0 + width * 0.55 + relief * relief_scale).clamp(7.0, 64.0)
    }

    fn bank_height_variation(
        &self,
        wx: i32,
        wz: i32,
        relief: f32,
        steepness: f32,
        bank_t: f32,
    ) -> f32 {
        let envelope = smoothstep(0.08, 0.36, bank_t) * (1.0 - smoothstep(0.86, 1.0, bank_t));
        if envelope <= 0.0 {
            return 0.0;
        }

        let broad = self
            .bank
            .get([wx as f64 * 0.041 - 177.0, wz as f64 * 0.041 + 53.0]) as f32;
        let detail = self
            .bank
            .get([wx as f64 * 0.137 + 31.0, wz as f64 * 0.137 - 211.0]) as f32;
        let terrace =
            self.bank
                .get([wx as f64 * 0.083 + 307.0, wz as f64 * 0.083 + 149.0]) as f32;
        let amplitude = (1.3 + relief * 0.065 + (1.0 - steepness) * 1.1).clamp(1.0, 6.5);
        let signal = broad * 0.65 + detail * 0.25 + terrace.signum() * 0.18;
        signal * amplitude * envelope
    }

    fn bed_block(&self, wx: i32, wz: i32, biome: Biome) -> Block {
        let material_noise = self.material.get([wx as f64 * 0.0045, wz as f64 * 0.0045]) as f32;
        let sand_bias = match biome {
            Biome::Ocean | Biome::DeepOcean | Biome::Beach | Biome::Desert => 0.72,
            Biome::Badlands | Biome::Savanna => 0.48,
            Biome::Swamp | Biome::Wetland => 0.12,
            Biome::Mountains | Biome::SnowyPeaks | Biome::StonyPeaks | Biome::WindsweptHills => {
                -0.12
            }
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

    fn bank_block(
        &self,
        wx: i32,
        wz: i32,
        biome: Biome,
        influence: f32,
        width: f32,
    ) -> Option<Block> {
        let deposit_noise =
            self.material
                .get([wx as f64 * 0.0065 + 211.0, wz as f64 * 0.0065 - 109.0]) as f32
                * 0.5
                + 0.5;
        let zone = smoothstep(0.22, 0.82, influence) * smoothstep(4.0, 15.0, width);
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

    fn flow_dir(&self, x: f32, z: f32, prev: (f32, f32)) -> (f32, f32) {
        const EPS: f32 = 64.0;
        let gx = self.potential(x + EPS, z) - self.potential(x - EPS, z);
        let gz = self.potential(x, z + EPS) - self.potential(x, z - EPS);
        let bend = self.bend.get([x as f64 * 0.0032, z as f64 * 0.0032]) as f32;
        let bend_dir = unit_from_angle(bend * std::f32::consts::TAU);
        normalize((
            -gx * 1.25 + bend_dir.0 * 0.58 + prev.0 * 0.72,
            -gz * 1.25 + bend_dir.1 * 0.58 + prev.1 * 0.72,
        ))
        .unwrap_or(prev)
    }

    fn potential(&self, x: f32, z: f32) -> f32 {
        self.potential.get([x as f64, z as f64]) as f32
            + (x * self.tilt_x + z * self.tilt_z) * 0.00085
    }
}

#[derive(Clone)]
struct RiverPath {
    points: Vec<RiverPoint>,
    min_x: f32,
    min_z: f32,
    max_x: f32,
    max_z: f32,
}

impl RiverPath {
    fn intersects(&self, x0: f32, z0: f32, x1: f32, z1: f32) -> bool {
        self.max_x >= x0 && self.min_x <= x1 && self.max_z >= z0 && self.min_z <= z1
    }
}

#[derive(Copy, Clone)]
struct RiverPoint {
    x: f32,
    z: f32,
    width: f32,
    depth: f32,
}

#[derive(Copy, Clone)]
struct RiverHit {
    distance: f32,
    width: f32,
    depth: f32,
}

fn nearest_hit(x: f32, z: f32, paths: &[RiverPath]) -> Option<RiverHit> {
    let mut best: Option<(f32, RiverHit)> = None;
    for path in paths {
        if x < path.min_x || x > path.max_x || z < path.min_z || z > path.max_z {
            continue;
        }
        for segment in path.points.windows(2) {
            let a = segment[0];
            let b = segment[1];
            let abx = b.x - a.x;
            let abz = b.z - a.z;
            let len2 = abx * abx + abz * abz;
            if len2 <= f32::EPSILON {
                continue;
            }
            let t = (((x - a.x) * abx + (z - a.z) * abz) / len2).clamp(0.0, 1.0);
            let px = a.x + abx * t;
            let pz = a.z + abz * t;
            let dx = x - px;
            let dz = z - pz;
            let distance = (dx * dx + dz * dz).sqrt();
            let width = a.width + (b.width - a.width) * t;
            if distance > MAX_QUERY_RADIUS {
                continue;
            }
            let hit = RiverHit {
                distance,
                width,
                depth: a.depth + (b.depth - a.depth) * t,
            };
            let score = distance / MAX_QUERY_RADIUS;
            if best.is_none_or(|(best_score, _)| score < best_score) {
                best = Some((score, hit));
            }
        }
    }
    best.map(|(_, hit)| hit)
}

#[inline]
fn end_taper(downstream: f32) -> f32 {
    smoothstep(0.0, 0.18, downstream) * (1.0 - smoothstep(0.82, 1.0, downstream))
}

#[inline]
fn unit_from_angle(angle: f32) -> (f32, f32) {
    (angle.cos(), angle.sin())
}

#[inline]
fn normalize(v: (f32, f32)) -> Option<(f32, f32)> {
    let len = (v.0 * v.0 + v.1 * v.1).sqrt();
    if len > 1e-5 {
        Some((v.0 / len, v.1 / len))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::classic::world::CascadeWorld;

    #[test]
    fn generated_rivers_use_one_fixed_water_level() {
        let world = CascadeWorld::new(12_345);
        let rivers = RiverSystem::new(12_345);
        let mut region = world.region(-512, -512, 1024, 1024);
        rivers.apply(&mut region);

        let active: Vec<_> = region.rivers.iter().filter(|r| r.wet()).collect();
        assert!(!active.is_empty(), "sample region should contain a river");
        assert!(
            active.iter().all(|r| r.water_y == SEA_LEVEL),
            "river water level must not vary by column"
        );

        let (min_w, max_w) = active.iter().fold((f32::MAX, f32::MIN), |(lo, hi), r| {
            (lo.min(r.width), hi.max(r.width))
        });
        let (min_d, max_d) = active.iter().fold((f32::MAX, f32::MIN), |(lo, hi), r| {
            (lo.min(r.depth), hi.max(r.depth))
        });
        assert!(
            max_w - min_w > 12.0,
            "river width should vary gradually along the path"
        );
        assert!(
            max_d - min_d > 3.0,
            "river depth should vary gradually along the path"
        );
    }

    #[test]
    fn generated_river_edges_have_water_level_shelf() {
        let world = CascadeWorld::new(12_345);
        let rivers = RiverSystem::new(12_345);
        let mut region = world.region(-512, -512, 1024, 1024);
        rivers.apply(&mut region);

        let mut shelf = 0usize;
        let mut water_level = 0usize;
        for (i, river) in region.rivers.iter().enumerate() {
            if river.preserve_bed || river.width < 8.0 || river.wet() {
                continue;
            }
            let wet_edge = river.width * 0.5 * 0.95;
            let from_edge = river.distance - wet_edge;
            if river.active() && (0.0..=1.5).contains(&from_edge) {
                shelf += 1;
                if region.surf[i] == SEA_LEVEL {
                    water_level += 1;
                }
            }
        }

        assert!(shelf > 64, "sample should contain measurable river shelves");
        assert!(
            water_level as f32 / shelf as f32 > 0.90,
            "blocks immediately outside the wet channel should usually sit at water level"
        );
    }

    #[test]
    fn bank_carve_fluctuates_around_smoothed_terrain() {
        let rivers = RiverSystem::new(12_345);
        let paths = [RiverPath {
            points: vec![
                RiverPoint {
                    x: -384.0,
                    z: 0.0,
                    width: 24.0,
                    depth: 6.0,
                },
                RiverPoint {
                    x: 384.0,
                    z: 0.0,
                    width: 24.0,
                    depth: 6.0,
                },
            ],
            min_x: -384.0 - MAX_QUERY_RADIUS,
            min_z: -MAX_QUERY_RADIUS,
            max_x: 384.0 + MAX_QUERY_RADIUS,
            max_z: MAX_QUERY_RADIUS,
        }];

        let mut min_y = i32::MAX;
        let mut max_y = i32::MIN;
        let mut samples = 0usize;
        for wx in (-320..=320).step_by(8) {
            let Some((river, carved_surf)) =
                rivers.carve_column(wx, 30, SEA_LEVEL + 32, Biome::Plains, &paths)
            else {
                continue;
            };
            assert!(river.active());
            assert!(!river.wet());
            assert!(
                (SEA_LEVEL..=SEA_LEVEL + 32).contains(&carved_surf),
                "bank variation should still stay between water level and the pre-river terrain"
            );
            min_y = min_y.min(carved_surf);
            max_y = max_y.max(carved_surf);
            samples += 1;
        }

        assert!(samples > 32, "synthetic bank should produce enough samples");
        assert!(
            max_y - min_y >= 3,
            "constant-height input terrain should still produce varied river bank heights"
        );
    }

    #[test]
    fn generated_paths_taper_at_both_ends() {
        let rivers = RiverSystem::new(12_345);
        let mut path = None;
        'search: for cz in -8..=8 {
            for cx in -8..=8 {
                if let Some(found) = rivers.path_from_cell(cx, cz) {
                    path = Some(found);
                    break 'search;
                }
            }
        }
        let path = path.expect("search area should contain a generated river path");

        let first = path.points.first().unwrap();
        let mid = path.points[path.points.len() / 2];
        let last = path.points.last().unwrap();
        assert!(first.width < 0.5, "source end should start essentially dry");
        assert!(last.width < 0.5, "far end should taper out");
        assert!(
            mid.width > 12.0,
            "middle of a generated river should be visibly wide"
        );
        assert!(first.depth < 0.5 && last.depth < 0.5);
        assert!(mid.depth > 3.0);
    }

    #[test]
    fn grass_biome_exposed_banks_are_sparse_non_dirt_deposits() {
        let world = CascadeWorld::new(12_345);
        let rivers = RiverSystem::new(12_345);
        let mut region = world.region(-512, -512, 1024, 1024);
        rivers.apply(&mut region);

        let mut candidates = 0usize;
        let mut deposits = 0usize;
        let mut brown_deposits = 0usize;
        for (i, river) in region.rivers.iter().enumerate() {
            if !river.active() || river.wet() || river.preserve_bed || river.influence < 0.35 {
                continue;
            }
            let biome = map_biome(region.biome_ids[i]);
            if !matches!(
                biome,
                Biome::Plains
                    | Biome::Meadow
                    | Biome::Forest
                    | Biome::BirchForest
                    | Biome::DarkForest
                    | Biome::Jungle
                    | Biome::CherryGrove
                    | Biome::Taiga
                    | Biome::OldGrowthTaiga
            ) {
                continue;
            }

            candidates += 1;
            if let Some(block) = river.bank_block {
                deposits += 1;
                if matches!(block, Block::Dirt | Block::CoarseDirt) {
                    brown_deposits += 1;
                }
            }
        }

        assert!(
            candidates > 128,
            "sample should contain exposed grass-biome river banks"
        );
        assert_eq!(
            brown_deposits, 0,
            "grass-biome exposed bank deposits should not be dirt"
        );
        assert!(
            deposits as f32 / (candidates as f32) < 0.35,
            "most grass-biome banks should keep the biome grass surface"
        );
    }

    #[test]
    fn mountainous_banks_bias_toward_steeper_carves() {
        let rivers = RiverSystem::new(12_345);
        let plains = rivers.bank_steepness(0, 0, Biome::Plains, 10.0);
        let mountains = rivers.bank_steepness(0, 0, Biome::Mountains, 80.0);
        assert!(
            mountains > plains,
            "mountainous terrain should bias toward steeper river banks"
        );

        let gentle_extra = rivers.bank_extra(20.0, 80.0, 0.15, false);
        let steep_extra = rivers.bank_extra(20.0, 80.0, 0.9, false);
        assert!(
            steep_extra < gentle_extra,
            "steeper banks should use less horizontal runout"
        );
    }

    #[test]
    fn bedding_material_follows_biome_context() {
        let rivers = RiverSystem::new(12_345);
        let mut desert_sand = 0usize;
        let mut plains_sand = 0usize;
        let mut plains_soil = 0usize;
        let mut mountain_gravel = 0usize;

        for z in 0..48 {
            for x in 0..48 {
                let wx = x * 19 - 380;
                let wz = z * 23 - 540;
                if rivers.bed_block(wx, wz, Biome::Desert) == Block::Sand {
                    desert_sand += 1;
                }
                match rivers.bed_block(wx, wz, Biome::Plains) {
                    Block::Sand => plains_sand += 1,
                    Block::Dirt | Block::CoarseDirt => plains_soil += 1,
                    _ => {}
                }
                if rivers.bed_block(wx, wz, Biome::Mountains) == Block::Gravel {
                    mountain_gravel += 1;
                }
            }
        }

        assert!(
            desert_sand > plains_sand * 3,
            "desert/coastal-like contexts should strongly prefer sand bedding"
        );
        assert!(
            plains_soil > plains_sand,
            "grass-dominant contexts should prefer soil bedding over sand"
        );
        assert!(
            mountain_gravel > plains_sand,
            "mountain contexts should expose more gravelly bedding"
        );
    }

    #[test]
    fn river_through_existing_water_preserves_bed_material_flag() {
        let rivers = RiverSystem::new(7);
        let path = RiverPath {
            points: vec![
                RiverPoint {
                    x: -32.0,
                    z: 0.0,
                    width: 16.0,
                    depth: 5.0,
                },
                RiverPoint {
                    x: 32.0,
                    z: 0.0,
                    width: 18.0,
                    depth: 5.5,
                },
            ],
            min_x: -MAX_QUERY_RADIUS,
            min_z: -MAX_QUERY_RADIUS,
            max_x: MAX_QUERY_RADIUS,
            max_z: MAX_QUERY_RADIUS,
        };

        let (river, carved_surf) = rivers
            .carve_column(0, 0, SEA_LEVEL - 1, Biome::Plains, &[path])
            .expect("centerline should carve the shallow water-body floor");

        assert!(river.wet());
        assert!(river.preserve_bed);
        assert_eq!(river.water_y, SEA_LEVEL);
        assert!(
            carved_surf < SEA_LEVEL - 1,
            "river should clear shallow water-body floors to its channel bed"
        );
    }
}
