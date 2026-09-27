use serde::Deserialize;

use crate::noise::settings::{CAVE_ENTRANCE_MAX_DEPTH, CAVE_MIN_Y, CAVE_SURFACE_BUFFER};
use petramond_world::block::Block;
use petramond_world::chunk::WORLD_MAX_Y;

pub const POOL_CELL: i32 = 16;

#[derive(Clone, Copy, Debug)]
pub struct FluidPool {
    pub name: &'static str,
    pub fluid: u16,
    pub anchor_y: i32,
    pub chance: f32,
    pub height_scale: f32,
    pub max_y: i32,
    pub reach: i32,
    pub max_depth: i32,
    pub max_drop: i32,
    pub max_sink: i32,
    pub budget: usize,
    pub surface_clearance: i32,
    pub salt: u64,
}

impl FluidPool {
    pub fn reach_up(&self) -> [i32; 3] {
        [
            POOL_CELL - 1 + self.reach,
            POOL_CELL - 1 + self.max_depth,
            POOL_CELL - 1 + self.reach,
        ]
    }

    pub fn reach_down(&self) -> i32 {
        POOL_CELL - 1 + self.max_drop + self.max_sink
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FluidFall {
    pub name: &'static str,
    pub fluid: u16,
    pub chance: f32,
    pub y: (i32, i32),
    pub min_surface: i32,
    pub salt: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawPool {
    fluid_pool: String,
    fluid: Block,
    anchor_y: i32,
    chance: f32,
    height_scale: f32,
    max_y: i32,
    reach: i32,
    max_depth: i32,
    max_drop: i32,
    max_sink: i32,
    budget: usize,
    surface_clearance: i32,
    #[serde(default)]
    salt: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawFall {
    fluid_fall: String,
    fluid: Block,
    chance: f32,
    y: [i32; 2],
    min_surface: i32,
    #[serde(default)]
    salt: Option<u64>,
}

pub(super) struct Rows {
    pub pools: Box<[FluidPool]>,
    pub falls: Box<[FluidFall]>,
}

pub(super) struct Layer {
    pub pools: Vec<RawPool>,
    pub falls: Vec<RawFall>,
}

pub(super) fn compile(layers: Vec<Layer>) -> Result<Rows, String> {
    let mut pools: Vec<RawPool> = Vec::new();
    let mut falls: Vec<RawFall> = Vec::new();
    for (li, layer) in layers.into_iter().enumerate() {
        merge(&mut pools, layer.pools, |r| &r.fluid_pool)
            .and_then(|()| merge(&mut falls, layer.falls, |r| &r.fluid_fall))
            .map_err(|e| format!("layer #{li}: {e}"))?;
    }
    let pools = pools.into_iter().map(pool).collect::<Result<Vec<_>, _>>()?;
    let falls = falls.into_iter().map(fall).collect::<Result<Vec<_>, _>>()?;
    Ok(Rows {
        pools: pools.into_iter().filter(|p| p.chance > 0.0).collect(),
        falls: falls.into_iter().filter(|f| f.chance > 0.0).collect(),
    })
}

fn merge<R>(into: &mut Vec<R>, rows: Vec<R>, key: fn(&R) -> &String) -> Result<(), String> {
    for row in rows {
        if !key(&row).contains(':') {
            return Err(format!("'{}': row keys must be namespaced", key(&row)));
        }
        match into.iter().position(|r| key(r) == key(&row)) {
            Some(i) => into[i] = row,
            None => into.push(row),
        }
    }
    Ok(())
}

fn leak(name: String) -> &'static str {
    Box::leak(name.into_boxed_str())
}

fn salt_for(kind: &str, name: &str, salt: Option<u64>) -> u64 {
    salt.unwrap_or_else(|| crate::salts::named(kind, name))
}

fn fluid_id(fluid: Block, what: &str) -> Result<u16, String> {
    if fluid.is_fluid() {
        Ok(fluid.id())
    } else {
        Err(format!("{what}: 'fluid' must name a fluid block"))
    }
}

fn pool(r: RawPool) -> Result<FluidPool, String> {
    let what = format!("fluid pool '{}'", r.fluid_pool);
    let fluid = fluid_id(r.fluid, &what)?;
    let checks: [(bool, &str); 10] = [
        (
            (0.0..=1.0).contains(&r.chance),
            "'chance' must lie in [0, 1]",
        ),
        (
            (CAVE_MIN_Y..=r.max_y).contains(&r.anchor_y),
            "'anchor_y' must lie between the carve floor and 'max_y'",
        ),
        (
            r.height_scale.is_finite() && r.height_scale > 0.0,
            "'height_scale' must be positive",
        ),
        (
            (CAVE_MIN_Y..WORLD_MAX_Y).contains(&r.max_y),
            "'max_y' must lie in the carvable world",
        ),
        ((1..=64).contains(&r.reach), "'reach' must lie in [1, 64]"),
        (
            (1..=64).contains(&r.max_depth),
            "'max_depth' must lie in [1, 64]",
        ),
        (
            (1..=64).contains(&r.max_drop),
            "'max_drop' must lie in [1, 64]",
        ),
        (
            (0..=64).contains(&r.max_sink),
            "'max_sink' must lie in [0, 64]",
        ),
        (
            (1..=65_536).contains(&r.budget),
            "'budget' must lie in [1, 65536]",
        ),
        (
            r.surface_clearance >= 0,
            "'surface_clearance' must not be negative",
        ),
    ];
    if let Some((_, err)) = checks.iter().find(|(ok, _)| !ok) {
        return Err(format!("{what}: {err}"));
    }
    let salt = salt_for("fluid_pool", &r.fluid_pool, r.salt);
    Ok(FluidPool {
        name: leak(r.fluid_pool),
        fluid,
        anchor_y: r.anchor_y,
        chance: r.chance,
        height_scale: r.height_scale,
        max_y: r.max_y,
        reach: r.reach,
        max_depth: r.max_depth,
        max_drop: r.max_drop,
        max_sink: r.max_sink,
        budget: r.budget,
        surface_clearance: r.surface_clearance,
        salt,
    })
}

fn fall(r: RawFall) -> Result<FluidFall, String> {
    let what = format!("fluid fall '{}'", r.fluid_fall);
    let fluid = fluid_id(r.fluid, &what)?;
    let [lo, hi] = r.y;
    if !(0.0..=1.0).contains(&r.chance) {
        return Err(format!("{what}: 'chance' must lie in [0, 1]"));
    }
    if lo <= CAVE_MIN_Y || lo > hi {
        return Err(format!(
            "{what}: 'y' must be increasing and above the carve floor {CAVE_MIN_Y}"
        ));
    }
    if r.min_surface >= WORLD_MAX_Y {
        return Err(format!(
            "{what}: 'min_surface' must lie under the world top {WORLD_MAX_Y}"
        ));
    }
    if r.min_surface - CAVE_SURFACE_BUFFER < hi + 1
        || r.min_surface - (hi + 1) <= CAVE_ENTRANCE_MAX_DEPTH
    {
        return Err(format!(
            "{what}: 'min_surface' must stand more than {CAVE_ENTRANCE_MAX_DEPTH} blocks above the band"
        ));
    }
    let salt = salt_for("fluid_fall", &r.fluid_fall, r.salt);
    Ok(FluidFall {
        name: leak(r.fluid_fall),
        fluid,
        chance: r.chance,
        y: (lo, hi),
        min_surface: r.min_surface,
        salt,
    })
}

pub(super) fn fingerprint(rows: &Rows, eat: &mut impl FnMut(&[u8])) {
    let name = |id: u16| {
        petramond_world::registry::names()
            .blocks
            .name(id)
            .unwrap_or("?")
    };
    for p in rows.pools.iter() {
        eat(p.name.as_bytes());
        eat(name(p.fluid).as_bytes());
        for v in [
            p.anchor_y,
            p.max_y,
            p.reach,
            p.max_depth,
            p.max_drop,
            p.max_sink,
            p.budget as i32,
            p.surface_clearance,
        ] {
            eat(&v.to_le_bytes());
        }
        eat(&p.chance.to_le_bytes());
        eat(&p.height_scale.to_le_bytes());
        eat(&p.salt.to_le_bytes());
    }
    for f in rows.falls.iter() {
        eat(f.name.as_bytes());
        eat(name(f.fluid).as_bytes());
        for v in [f.y.0, f.y.1, f.min_surface] {
            eat(&v.to_le_bytes());
        }
        eat(&f.chance.to_le_bytes());
        eat(&f.salt.to_le_bytes());
    }
}
