use mod_sdk::*;

use crate::keys;

const SALT_FIELD: u64 = 0xF012_C1A7_0000_0001;
const SALT_DEPTH: u64 = 0xF012_C1A7_0000_0002;

const PERIOD: i32 = 24;

const RIVER_MIN: f32 = 0.70;
const BANK_MIN: f32 = 0.70;
const SAVANNA_MIN: f32 = 0.84;

const BANK_REACH: i32 = 4;
const PROBES: [(i32, i32); 4] = [
    (BANK_REACH, 0),
    (-BANK_REACH, 0),
    (0, BANK_REACH),
    (0, -BANK_REACH),
];

const DEPTH_MIN: i32 = 1;
const DEPTH_MAX: i32 = 3;

pub(crate) const GEN_FILTER: GenFeatureFilter = GenFeatureFilter::surface_band(1 - DEPTH_MAX, 0);

const BANK_MAX_RISE: i32 = 6;

const REPLACEABLE: [&str; 8] = [
    keys::GRASS,
    keys::DIRT,
    keys::COARSE_DIRT,
    keys::PODZOL,
    keys::SAND,
    keys::RED_SAND,
    keys::GRAVEL,
    keys::STONE,
];

#[derive(Default)]
pub struct Deposits {
    clay: Option<BlockId>,
    replaceable: Vec<BlockId>,
}

impl Deposits {
    pub fn init(&mut self) {
        self.clay = resolve_block_logged(keys::CLAY_BLOCK);
        self.replaceable = REPLACEABLE
            .iter()
            .filter_map(|n| resolve_block_logged(n))
            .collect();
    }

    pub fn generate(&self, ctx: &GenCtx) -> Vec<GenWrite> {
        let Some(clay) = self.clay else {
            return Vec::new();
        };
        let origin = ctx.origin_world();
        let (y0, y1) = (origin[1], origin[1] + 16);
        let seed = ctx.seed();

        let field = FieldTile::over(seed, origin[0], origin[2]);
        let mut accepted: Vec<Column> = Vec::new();
        let mut probing: Vec<Column> = Vec::new();
        ctx.for_each_origin(0, |wx, wz| {
            let Some(surf) = ctx.surface_y(wx, wz) else {
                return;
            };
            if surf < y0 || surf - DEPTH_MAX + 1 >= y1 {
                return;
            }
            let Some(biome) = ctx.biome(wx, wz) else {
                return;
            };
            let roll = field.at(wx, wz);
            let column = Column { wx, wz, surf };
            match biome {
                biome::RIVER if roll >= RIVER_MIN => accepted.push(column),
                biome::SAVANNA if roll >= SAVANNA_MIN => accepted.push(column),
                b if roll >= BANK_MIN && bankable(b) && surf <= ctx.sea_level() + BANK_MAX_RISE => {
                    probing.push(column)
                }
                _ => {}
            }
        });

        if !probing.is_empty() {
            let mut columns = Vec::with_capacity(probing.len() * PROBES.len());
            for c in &probing {
                for (dx, dz) in PROBES {
                    columns.push([c.wx + dx, c.wz + dz]);
                }
            }
            let biomes = surface_biome_at(columns);
            for (i, column) in probing.into_iter().enumerate() {
                let ring = &biomes[i * PROBES.len()..(i + 1) * PROBES.len()];
                if ring.contains(&biome::RIVER) {
                    accepted.push(column);
                }
            }
        }

        let mut writes = Vec::new();
        for column in accepted {
            let mut rng = GenRng::positional(seed, SALT_DEPTH, column.wx, 0, column.wz);
            let depth = rng.next_i32(DEPTH_MIN, DEPTH_MAX);
            for y in (column.surf - depth + 1)..=column.surf {
                if y < y0 || y >= y1 {
                    continue;
                }
                let pos = [column.wx, y, column.wz];
                if ctx
                    .block(pos)
                    .is_some_and(|b| self.replaceable.contains(&b))
                {
                    writes.push((pos, clay));
                }
            }
        }
        writes
    }
}

#[derive(Clone, Copy)]
struct Column {
    wx: i32,
    wz: i32,
    surf: i32,
}

fn bankable(b: u8) -> bool {
    !matches!(
        b,
        biome::OCEAN
            | biome::DEEP_OCEAN
            | biome::RIVER
            | biome::SNOWY_PEAKS
            | biome::STONY_PEAKS
            | biome::MOUNTAINS
    )
}

struct FieldTile {
    cx: i32,
    cz: i32,
    corners: Vec<f32>,
    stride: usize,
}

impl FieldTile {
    fn over(seed: u32, x0: i32, z0: i32) -> FieldTile {
        let cx = x0.div_euclid(PERIOD);
        let cz = z0.div_euclid(PERIOD);
        let span_x = ((x0 + 15).div_euclid(PERIOD) - cx + 1) as usize + 1;
        let span_z = ((z0 + 15).div_euclid(PERIOD) - cz + 1) as usize + 1;
        let mut corners = Vec::with_capacity(span_x * span_z);
        for iz in 0..span_z as i32 {
            for ix in 0..span_x as i32 {
                corners.push(GenRng::positional(seed, SALT_FIELD, cx + ix, 0, cz + iz).next_f32());
            }
        }
        FieldTile {
            cx,
            cz,
            corners,
            stride: span_x,
        }
    }

    fn at(&self, wx: i32, wz: i32) -> f32 {
        let ix = (wx.div_euclid(PERIOD) - self.cx) as usize;
        let iz = (wz.div_euclid(PERIOD) - self.cz) as usize;
        let sx = smoothstep01(wx.rem_euclid(PERIOD) as f32 / PERIOD as f32);
        let sz = smoothstep01(wz.rem_euclid(PERIOD) as f32 / PERIOD as f32);
        let corner = |dx: usize, dz: usize| self.corners[(iz + dz) * self.stride + ix + dx];
        let top = corner(0, 0) + (corner(1, 0) - corner(0, 0)) * sx;
        let bottom = corner(0, 1) + (corner(1, 1) - corner(0, 1)) * sx;
        top + (bottom - top) * sz
    }
}
