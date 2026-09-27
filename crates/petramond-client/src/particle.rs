use petramond_render::atlas;

use petramond::world::ReplicaWorld;
use petramond_math::math::{IVec3, Vec3};
use petramond_math::world_pos::WorldPos;
use petramond_world::biome::Biome;
use petramond_world::block::Block;
use petramond_world::block_model::{self, BlockModelKind};
use petramond_world::tile::Tile;

use petramond::entity::hash01;

const NO_TINT: [f32; 3] = [1.0, 1.0, 1.0];

fn mul_kv_tint(tint: [f32; 3], kv: Option<[u8; 3]>) -> [f32; 3] {
    match kv {
        Some([r, g, b]) => [
            tint[0] * r as f32 / 255.0,
            tint[1] * g as f32 / 255.0,
            tint[2] * b as f32 / 255.0,
        ],
        None => tint,
    }
}

#[inline]
fn tile_tint(tile: Tile) -> [f32; 3] {
    match tile.icon_tint() {
        Some(petramond_world::tile::TileTint::Grass) => Biome::PLAINS.grass_color(),
        Some(petramond_world::tile::TileTint::Foliage) => Biome::PLAINS.foliage_color(),
        Some(petramond_world::tile::TileTint::Fixed(rgb)) => rgb.map(|c| f32::from(c) / 255.0),
        _ => NO_TINT,
    }
}

const PARTICLE_GRAVITY: f32 = -12.0;
const FADE_TAIL: f32 = 0.4;

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ParticleSource {
    Solid,
    Tile { tile: Tile, dyed: bool },
    Model(BlockModelKind),
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Particle {
    pub pos: WorldPos,
    pub vel: Vec3,
    pub skylight: u8,
    pub blocklight: petramond_world::light::BlockLight6,
    pub source: ParticleSource,
    pub uv_min: [f32; 2],
    pub uv_size: [f32; 2],
    pub tint: [f32; 3],
    pub die_on_contact: bool,
    pub age: f32,
    pub lifetime: f32,
    pub size: f32,
}

impl Particle {
    #[inline]
    pub fn atlas_uv(&self) -> ([f32; 2], [f32; 2]) {
        let ParticleSource::Tile { tile, dyed } = self.source else {
            return (self.uv_min, self.uv_size);
        };
        let [u0, v0, u1, v1] = atlas::tile_uv(tile);
        let tw = u1 - u0;
        let th = v1 - v0;
        let dye = if dyed { atlas::DYE_V_OFFSET } else { 0.0 };
        let inset = 0.5 / atlas::TILE as f32;
        let span = 1.0 - 2.0 * inset;
        let abs_min = [
            u0 + (inset + self.uv_min[0] * span) * tw,
            v0 + dye + (inset + self.uv_min[1] * span) * th,
        ];
        let abs_size = [self.uv_size[0] * span * tw, self.uv_size[1] * span * th];
        (abs_min, abs_size)
    }

    #[inline]
    pub fn alpha(&self) -> f32 {
        if self.lifetime <= 0.0 {
            return 0.0;
        }
        let t = (self.age / self.lifetime).clamp(0.0, 1.0);
        if t <= 1.0 - FADE_TAIL {
            1.0
        } else {
            ((1.0 - t) / FADE_TAIL).clamp(0.0, 1.0)
        }
    }

    #[inline]
    pub fn render_size(&self) -> f32 {
        self.size * self.alpha()
    }

    #[inline]
    fn is_dead(&self) -> bool {
        self.age >= self.lifetime
    }
}

pub struct ParticleSystem {
    particles: Vec<Particle>,
    seed: u64,
    count_scale: f32,
}

const POOL_WARM_CAPACITY: usize = 4096;

impl ParticleSystem {
    pub fn new() -> Self {
        ParticleSystem {
            particles: Vec::with_capacity(POOL_WARM_CAPACITY),
            seed: 0,
            count_scale: 1.0,
        }
    }

    pub fn clear(&mut self) {
        self.particles.clear();
    }

    pub fn set_count_scale(&mut self, scale: f32) {
        self.count_scale = scale.clamp(0.0, 1.0);
    }

    pub fn count_scale(&self) -> f32 {
        self.count_scale
    }

    fn scaled_count(&self, count: usize) -> usize {
        if self.count_scale <= 0.0 {
            return 0;
        }
        ((count as f32 * self.count_scale).ceil() as usize).min(count)
    }

    #[cfg(test)]
    #[inline]
    pub fn len(&self) -> usize {
        self.particles.len()
    }

    #[cfg(test)]
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.particles.is_empty()
    }

    #[inline]
    pub fn particles(&self) -> &[Particle] {
        &self.particles
    }

    pub fn tick(&mut self, dt: f32, world: &ReplicaWorld) {
        self.tick_with(dt, &|p| world.data().point_blocked(p), &|p| {
            let c = p.block();
            world.data().fluid_cell_at(c.x, c.y, c.z)
        });
        for p in &mut self.particles {
            let c = p.pos.block();
            let (sky, block) = world.data().dynamic_light_at_world(c.x, c.y, c.z);
            p.skylight = sky;
            p.blocklight = block;
        }
    }

    fn tick_with(
        &mut self,
        dt: f32,
        blocked: &impl Fn(WorldPos) -> bool,
        fluid: &impl Fn(WorldPos) -> bool,
    ) {
        let mut i = 0;
        while i < self.particles.len() {
            let p = &mut self.particles[i];
            p.age += dt;
            p.vel.y += PARTICLE_GRAVITY * dt;
            let next = p.pos + p.vel * dt;
            if p.die_on_contact && (blocked(next) || fluid(next)) {
                p.age = p.lifetime;
            } else if blocked(next) {
                p.vel = Vec3::ZERO;
            } else {
                p.pos = next;
            }

            if self.particles[i].is_dead() {
                self.particles.swap_remove(i);
            } else {
                i += 1;
            }
        }
    }

    #[inline]
    fn push(&mut self, p: Particle) {
        self.particles.push(p);
    }

    #[inline]
    fn rand(&mut self) -> f32 {
        self.seed = self.seed.wrapping_add(1);
        hash01(self.seed)
    }

    pub fn spawn_burst(
        &mut self,
        spec: &petramond_world::particle_emitters::BurstSpec,
        event: BurstEvent,
    ) {
        let base = spec.count_per_intensity * event.intensity.max(0.0);
        let count = (base * (1.0 + self.rand() * spec.count_spread)).round() as u32;
        let count = self.scaled_count(count.max(1) as usize);
        let recolour = event.look == BurstLook::Row;
        let look = match event.look {
            BurstLook::Row => spec
                .texture
                .map_or(BurstLook::Row, |slice| BurstLook::Texture {
                    slice,
                    tint: NO_TINT,
                }),
            look => look,
        };
        let along = event.direction.map(|d| d.normalize_or_zero());
        let pick = |range: [f32; 2], r: f32| range[0] + r * (range[1] - range[0]);
        for _ in 0..count {
            let offset = Vec3::new(
                (self.rand() - 0.5) * 2.0 * spec.spawn[0],
                (self.rand() - 0.5) * 2.0 * spec.spawn[1],
                (self.rand() - 0.5) * 2.0 * spec.spawn[2],
            );
            let angle = self.rand() * std::f32::consts::TAU;
            let radial = pick(spec.radial_speed, self.rand());
            let mut vel = Vec3::new(
                angle.cos() * radial,
                pick(spec.up_speed, self.rand()),
                angle.sin() * radial,
            );
            vel += offset.normalize_or_zero() * pick(spec.outward_speed, self.rand());
            if let Some(along) = along {
                vel += along * pick(spec.along_speed, self.rand());
            }
            let mix = self.rand().powf(spec.color_bias);
            let color: [f32; 3] = std::array::from_fn(|c| {
                spec.color[0][c] + (spec.color[1][c] - spec.color[0][c]) * mix
            });
            let lifetime = pick(spec.lifetime, self.rand());
            let size = pick(spec.size, self.rand());
            let skin = self.skin(look, along, spec.patch);
            self.push(Particle {
                pos: event.pos + offset,
                vel,
                skylight: event.skylight.min(63),
                blocklight: event.blocklight,
                source: skin.source,
                uv_min: skin.uv_min,
                uv_size: skin.uv_size,
                tint: if recolour {
                    std::array::from_fn(|c| skin.tint[c] * color[c])
                } else {
                    skin.tint
                },
                die_on_contact: spec.die_on_contact,
                age: 0.0,
                lifetime,
                size,
            });
        }
    }

    fn skin(&mut self, look: BurstLook, along: Option<Vec3>, patch: f32) -> Skin {
        match look {
            BurstLook::Row => Skin {
                source: ParticleSource::Solid,
                uv_min: [0.0; 2],
                uv_size: [patch; 2],
                tint: NO_TINT,
            },
            BurstLook::Texture { slice, tint } => {
                let [u0, v0, u1, v1] = slice.slice;
                let size = [(u1 - u0) * patch, (v1 - v0) * patch];
                let uv_min = [
                    u0 + self.rand() * (u1 - u0 - size[0]),
                    v0 + self.rand() * (v1 - v0 - size[1]),
                ];
                let base = tile_tint(slice.tile);
                Skin {
                    source: ParticleSource::Tile {
                        tile: slice.tile,
                        dyed: false,
                    },
                    uv_min,
                    uv_size: size,
                    tint: std::array::from_fn(|c| base[c] * tint[c]),
                }
            }
            BurstLook::Block { block, kv_tint } => {
                if let Some(kind) = block.model_kind() {
                    let (uv_min, uv_size) = block_model::particle_patch(kind, self.rand());
                    return Skin {
                        source: ParticleSource::Model(kind),
                        uv_min,
                        uv_size,
                        tint: NO_TINT,
                    };
                }
                let tiles = block.tiles();
                let tile = match along {
                    Some(d) if d.y.abs() >= d.x.abs().max(d.z.abs()) => {
                        tiles[if d.y > 0.0 { 0 } else { 1 }]
                    }
                    Some(_) => tiles[2],
                    None if self.rand() < 0.3 => tiles[0],
                    None => tiles[2],
                };
                let span = 1.0 - patch;
                Skin {
                    source: ParticleSource::Tile {
                        tile,
                        dyed: kv_tint.is_some(),
                    },
                    uv_min: [self.rand() * span, self.rand() * span],
                    uv_size: [patch; 2],
                    tint: mul_kv_tint(tile_tint(tile), kv_tint),
                }
            }
        }
    }
}

pub const BLOCK_DUST: &str = "petramond:block_dust";
pub const BLOCK_BREAK: &str = "petramond:block_break";

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum BurstLook {
    Row,
    Texture {
        slice: petramond_world::particle_emitters::TextureSlice,
        tint: [f32; 3],
    },
    Block {
        block: Block,
        kv_tint: Option<[u8; 3]>,
    },
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BurstEvent {
    pub pos: WorldPos,
    pub intensity: f32,
    pub direction: Option<Vec3>,
    pub look: BurstLook,
    pub skylight: u8,
    pub blocklight: petramond_world::light::BlockLight6,
}

impl BurstEvent {
    pub fn struck(
        cell: IVec3,
        normal: IVec3,
        block: Block,
        kv_tint: Option<[u8; 3]>,
        skylight: u8,
        blocklight: petramond_world::light::BlockLight6,
    ) -> Self {
        let n = normal.as_vec3();
        Self {
            pos: WorldPos::block_center(cell) + n * 0.55,
            intensity: 1.0,
            direction: Some(n),
            look: BurstLook::Block { block, kv_tint },
            skylight,
            blocklight,
        }
    }

    pub fn broken(
        cell: IVec3,
        block: Block,
        kv_tint: Option<[u8; 3]>,
        skylight: u8,
        blocklight: petramond_world::light::BlockLight6,
    ) -> Self {
        Self {
            pos: WorldPos::block_center(cell),
            intensity: 1.0,
            direction: None,
            look: BurstLook::Block { block, kv_tint },
            skylight,
            blocklight,
        }
    }
}

struct Skin {
    source: ParticleSource,
    uv_min: [f32; 2],
    uv_size: [f32; 2],
    tint: [f32; 3],
}

impl Default for ParticleSystem {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
