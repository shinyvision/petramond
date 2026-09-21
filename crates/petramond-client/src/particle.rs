//! Short-lived terrain particles (mining dust + block-break bursts).
//!
//! Stored in a fixed-capacity ring buffer so spawning never heap-allocates and
//! the per-frame integration touches a contiguous, bounded slice. Each particle
//! samples a small random-ish sub-patch of a block face tile, so a burst reads
//! as flecks of that block's texture.
//!
//! Render-agnostic: a particle exposes [`Particle::atlas_uv`] (absolute atlas
//! coords) and [`Particle::alpha`] (end-of-life fade); the App turns the alive
//! slice into render instances.

use petramond_render::atlas;

use petramond::world::World;
use petramond_math::math::{IVec3, Vec3};
use petramond_math::world_pos::WorldPos;
use petramond_world::biome::Biome;
use petramond_world::block::Block;
use petramond_world::block_model::{self, BlockModelKind};
use petramond_world::tile::Tile;

use petramond::entity::hash01;

/// White (no tint): the multiply identity for a fleck cut from an untinted tile.
const NO_TINT: [f32; 3] = [1.0, 1.0, 1.0];

/// Fold a cell's raw `petramond:tint` bytes into a fleck tint (multiply).
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

/// Foliage tint for a fleck cut from `tile`, mirroring the out-of-world tile
/// classification (the atlas manifest's `icon_tint`, defaulting to its in-world
/// `tint`) so a fleck of grass-top / short-grass / fern reads green and a fleck
/// of any leaf tile reads foliage-green; every other tile (dirt, stone, the
/// pre-baked grass-block *side*, logs, water, ...) stays untinted (white = no
/// change under the particle shader's multiply).
///
/// Render-agnostic on purpose: it uses only the low-level [`Tile`] / [`Biome`]
/// data — never `petramond_render` (see the module-level rule) — and, like the
/// icon/held-item path, picks the fixed temperate Plains colours since a fleck
/// has no biome context.
#[inline]
fn tile_tint(tile: Tile) -> [f32; 3] {
    match tile.icon_tint() {
        Some(petramond_world::tile::TileTint::Grass) => Biome::Plains.grass_color(),
        Some(petramond_world::tile::TileTint::Foliage) => Biome::Plains.foliage_color(),
        Some(petramond_world::tile::TileTint::Fixed(rgb)) => rgb.map(|c| f32::from(c) / 255.0),
        _ => NO_TINT,
    }
}

/// Downward acceleration on particles, m/s². Lighter than item gravity so dust
/// hangs a touch longer.
const PARTICLE_GRAVITY: f32 = -12.0;
/// Fraction of lifetime over which a particle fades out at the end.
const FADE_TAIL: f32 = 0.4;

/// What a particle is cut from.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum ParticleSource {
    /// Nothing: a flat alpha-blended cube of the particle's tint (a splash
    /// droplet), drawn with the looping-emitter particles.
    Solid,
    /// A sub-patch of a BLOCK-atlas tile, in `[0, 1]` tile fractions. `dyed`
    /// samples the tile's dye-base twin for a fleck carrying a cell tint.
    Tile { tile: Tile, dyed: bool },
    /// A bbmodel block's own texture: absolute MODEL-atlas coords, so a broken
    /// workbench throws workbench flecks.
    Model(BlockModelKind),
}

/// One terrain particle: a tiny textured quad sampling a sub-patch of `tile`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Particle {
    pub pos: WorldPos,
    pub vel: Vec3,
    /// 6-bit SKY light, re-sampled each tick so the fleck tracks the lighting it
    /// drifts through (and dims with the environment sky scale at night).
    pub skylight: u8,
    /// 6-bit COLOURED block light, re-sampled alongside `skylight` —
    /// night-invariant, so a fleck near a coloured lamp takes its hue.
    pub blocklight: petramond_world::light::BlockLight6,
    /// What the particle is cut from; decides the atlas it samples and what
    /// `uv_min`/`uv_size` mean.
    pub source: ParticleSource,
    /// Sub-tile patch origin in `[0, 1]` tile fractions (bottom-left) for a block fleck;
    /// the absolute model-atlas min for a model fleck.
    pub uv_min: [f32; 2],
    /// Per-axis patch extent: tile fractions (block) / absolute atlas units (model).
    /// Per-axis because neither atlas is square in normalized UV — the composed block
    /// atlas is double-height (dye twins), so a tile's V span is roughly half its U span.
    pub uv_size: [f32; 2],
    /// RGB tint multiplied into the fleck's atlas colour (foliage-green for a
    /// fleck cut from a grass/leaf tile, white otherwise). Classified per-fleck
    /// from [`tile`](Self::tile) so e.g. grass-top dust is green but the
    /// grass-block side/dirt dust is not. For a [`ParticleSource::Solid`]
    /// particle this IS the color.
    pub tint: [f32; 3],
    /// Destroyed the instant it touches a collision box OR a fluid, instead of
    /// settling on solids like terrain dust (burst rows opt in — a splash
    /// droplet vanishes into the pool it fell out of).
    pub die_on_contact: bool,
    pub age: f32,
    pub lifetime: f32,
    /// World-space quad edge length, metres.
    pub size: f32,
}

impl Particle {
    /// Absolute atlas UVs for this particle's sub-patch: `(uv_min, uv_size)` in
    /// normalized atlas space, ready for a render instance. Maps the sub-tile
    /// patch into the tile's rect from [`atlas::tile_uv`], inset half a texel
    /// per side so the sampled range stays STRICTLY inside the tile: an f32 sum
    /// landing exactly on a tile boundary resolves (nearest filtering) to the
    /// adjacent tile's texel — an iron-ore fleck flashing the flower tile next
    /// to it in the atlas. Tile edges are texel edges at every mip, so strictly
    /// inside at the base level is strictly inside at all levels.
    #[inline]
    pub fn atlas_uv(&self) -> ([f32; 2], [f32; 2]) {
        // A model fleck already carries absolute model-atlas coords, inset at scan
        // time (the render side binds the model atlas for these); a block fleck maps
        // its sub-patch into the block atlas tile rect.
        let ParticleSource::Tile { tile, dyed } = self.source else {
            return (self.uv_min, self.uv_size);
        };
        let [u0, v0, u1, v1] = atlas::tile_uv(tile);
        let tw = u1 - u0;
        let th = v1 - v0;
        // A dyed fleck samples the tile's dye-base twin (half a texture down).
        let dye = if dyed { atlas::DYE_V_OFFSET } else { 0.0 };
        // Half a texel in tile fractions; `span` maps `[0, 1]` into the inset rect.
        let inset = 0.5 / atlas::TILE as f32;
        let span = 1.0 - 2.0 * inset;
        let abs_min = [
            u0 + (inset + self.uv_min[0] * span) * tw,
            v0 + dye + (inset + self.uv_min[1] * span) * th,
        ];
        // Per-axis: the composed atlas is double-height, so th ≈ tw / 2 — scaling
        // both axes by tw would overshoot the tile bottom into the tile below.
        let abs_size = [self.uv_size[0] * span * tw, self.uv_size[1] * span * th];
        (abs_min, abs_size)
    }

    /// Normalized opacity in `[0, 1]`: full for most of the life, fading to 0 over
    /// the final `FADE_TAIL` fraction.
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

    /// World-space cube edge length for rendering, shrinking over the final
    /// `FADE_TAIL` fraction so a dying fleck visibly collapses to nothing. The
    /// cubes use an alpha CUTOUT (no smooth alpha fade), so shrinking is the fade
    /// cue; tracks the same curve as [`alpha`](Self::alpha).
    #[inline]
    pub fn render_size(&self) -> f32 {
        self.size * self.alpha()
    }

    /// `true` once the particle has outlived its lifetime.
    #[inline]
    fn is_dead(&self) -> bool {
        self.age >= self.lifetime
    }
}

/// The live particle pool: spawns append, ticks integrate and cull in place.
/// Bounded only by spawn rate × lifetime, never by a cap that recycles a
/// particle still in the air.
pub struct ParticleSystem {
    particles: Vec<Particle>,
    /// Monotonic counter feeding the deterministic hash for spawn variety.
    seed: u64,
    /// Spawn-count multiplier from the particles graphics option (`0` = off,
    /// `0.5` = reduced, `1` = full). Presentation-only; ticking existing
    /// particles is unaffected.
    count_scale: f32,
}

/// The pool's warm starting capacity — a first burst never reallocates its
/// way up from empty. A warm start, not a cap: the pool grows past it freely.
const POOL_WARM_CAPACITY: usize = 4096;

impl ParticleSystem {
    pub fn new() -> Self {
        ParticleSystem {
            particles: Vec::with_capacity(POOL_WARM_CAPACITY),
            seed: 0,
            count_scale: 1.0,
        }
    }

    pub fn set_count_scale(&mut self, scale: f32) {
        self.count_scale = scale.clamp(0.0, 1.0);
    }

    /// The particles graphics option's density (0 = off, 0.5 = reduced,
    /// 1 = full) — ambient volumes scale by the same knob.
    pub fn count_scale(&self) -> f32 {
        self.count_scale
    }

    /// Apply the particles-option multiplier to a spawn count. Full keeps
    /// `count`, reduced halves it (rounding up so small effects survive), off
    /// silences the spawn entirely.
    fn scaled_count(&self, count: usize) -> usize {
        if self.count_scale <= 0.0 {
            return 0;
        }
        ((count as f32 * self.count_scale).ceil() as usize).min(count)
    }

    /// Number of currently-alive particles.
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

    /// All alive particles. The App maps these to render instances (render-agnostic).
    #[inline]
    pub fn particles(&self) -> &[Particle] {
        &self.particles
    }

    /// Advance every particle by `dt`: gravity, integrate position with simple
    /// block-ground stop, age, then cull dead. Culling uses swap-remove so the
    /// live slice stays packed at the front.
    pub fn tick(&mut self, dt: f32, world: &World) {
        // Model-aware: a fleck settles on a bbmodel block's actual leg/top and drifts
        // through the empty space around it — the same `collision_boxes_at` shape source the
        // player/mob/item bodies collide against (here the point case, `World::point_blocked`).
        self.tick_with(dt, &|p| world.point_blocked(p), &|p| {
            let c = p.block();
            world.fluid_cell_at(c.x, c.y, c.z)
        });
        // Re-sample light each tick so a fleck dims/brightens as the lighting around
        // it changes (e.g. a torch broken in a dark cave), rather than staying frozen
        // at its spawn light.
        for p in &mut self.particles {
            let c = p.pos.block();
            let (sky, block) = world.dynamic_light_at_world(c.x, c.y, c.z);
            p.skylight = sky;
            p.blocklight = block;
        }
    }

    /// Pure tick behind [`tick`](Self::tick); `blocked(p)` reports whether a world point is
    /// inside a collision box (the model-aware shape) and `fluid(p)` whether it is inside
    /// a fluid cell, so tests can run without a `World`.
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
                // A splash droplet vanishes the instant it touches anything.
                p.age = p.lifetime;
            } else if blocked(next) {
                // Stop on a solid surface: if the next position lands inside a collision
                // box, kill velocity and pin in place so dust settles on the surface (not
                // the cell) rather than tunnelling through.
                p.vel = Vec3::ZERO;
            } else {
                p.pos = next;
            }

            if self.particles[i].is_dead() {
                self.particles.swap_remove(i);
                // Don't advance `i`: the swapped-in element needs processing.
            } else {
                i += 1;
            }
        }
    }

    #[inline]
    fn push(&mut self, p: Particle) {
        self.particles.push(p);
    }

    /// Next deterministic hash value in `[0, 1)`, advancing the internal counter.
    #[inline]
    fn rand(&mut self) -> f32 {
        self.seed = self.seed.wrapping_add(1);
        hash01(self.seed)
    }

    /// One burst: the ONE way particles enter the system. The bundle row
    /// (`spec`) owns how many there are and how they fly; `event` says where,
    /// how hard, which way, and what they are cut from. Mining dust, a block
    /// coming apart, a splash and a mod's burst differ only in their rows.
    pub fn spawn_burst(
        &mut self,
        spec: &petramond_world::particle_emitters::BurstSpec,
        event: BurstEvent,
    ) {
        let base = spec.count_per_intensity * event.intensity.max(0.0);
        let count = (base * (1.0 + self.rand() * spec.count_spread)).round() as u32;
        let count = self.scaled_count(count.max(1) as usize);
        // The row's colour is the row's look: what an event names to be cut
        // from keeps its own colours (grass flecks out of a brown burst).
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
            // The bias skews the endpoint mix: >1 spends more draws near 0,
            // making the FIRST endpoint the prominent one.
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

    /// What ONE particle of a burst shows.
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
                // Struck from one side, the flecks are that face's; coming
                // apart whole, a mix of its top and sides.
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
                    // Per fleck, by its tile: grass-top dust greens, the dirt
                    // side of the same block does not; a dyed cell multiplies.
                    tint: mul_kv_tint(tile_tint(tile), kv_tint),
                }
            }
        }
    }
}

/// The engine's own burst rows: what a struck block sheds, and a broken one.
pub const BLOCK_DUST: &str = "petramond:block_dust";
pub const BLOCK_BREAK: &str = "petramond:block_break";

/// What a burst's particles are cut from.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum BurstLook {
    /// The bundle row's own: its texture slice, else flat cubes of its colour.
    Row,
    /// Patches of a slice of an atlas tile, multiplied by `tint`.
    Texture {
        slice: petramond_world::particle_emitters::TextureSlice,
        tint: [f32; 3],
    },
    /// A block's own look: per particle one of its face tiles, or its model's
    /// texture, tinted as the block is (`kv_tint` = its cell's dye).
    Block {
        block: Block,
        kv_tint: Option<[u8; 3]>,
    },
}

/// One firing of a burst row.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BurstEvent {
    pub pos: WorldPos,
    pub intensity: f32,
    /// Which way the event pushes (a struck face's normal), for rows with an
    /// `along_speed`.
    pub direction: Option<Vec3>,
    pub look: BurstLook,
    pub skylight: u8,
    pub blocklight: petramond_world::light::BlockLight6,
}

impl BurstEvent {
    /// A block struck on the face `normal`: just outside that face, pushed
    /// along it.
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

    /// A block coming apart where it stood.
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
