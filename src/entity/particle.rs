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

use crate::atlas::{self, Tile};
use crate::biome::Biome;
use crate::block::Block;
use crate::mathh::{IVec3, Vec3};
use crate::world::World;

use super::hash01;

/// White (no tint): the multiply identity for a fleck cut from an untinted tile.
const NO_TINT: [f32; 3] = [1.0, 1.0, 1.0];

/// Foliage tint for a fleck cut from `tile`, mirroring the chunk mesher's tile
/// classification so a fleck of grass-top / short-grass / fern reads green and a
/// fleck of any `*Leaves` reads foliage-green; every other tile (dirt, stone, the
/// pre-baked grass-block *side*, logs, ...) stays untinted (white = no change
/// under the particle shader's multiply).
///
/// Render-agnostic on purpose: this is the tiny duplicate of the render-side
/// `foliage_tint::face_material` classification, kept here so `entity` never
/// imports `crate::render` (see the module-level rule). It uses only the
/// low-level [`Tile`] / [`Biome`] data, and — like the icon/held-item path — picks
/// the fixed temperate Plains colours since a fleck has no biome context.
#[inline]
fn tile_tint(tile: Tile) -> [f32; 3] {
    match tile {
        Tile::GrassTop | Tile::ShortGrass | Tile::Fern => Biome::Plains.grass_color(),
        Tile::OakLeaves
        | Tile::AcaciaLeaves
        | Tile::BirchLeaves
        | Tile::DarkOakLeaves
        | Tile::JungleLeaves
        | Tile::MangroveLeaves
        | Tile::SpruceLeaves
        | Tile::CherryLeaves
        | Tile::AzaleaLeaves => Biome::Plains.foliage_color(),
        _ => NO_TINT,
    }
}

/// Fixed particle pool size. Oldest particles are overwritten once full, which
/// is fine for transient dust — bursts at most spend a few dozen slots.
pub const PARTICLE_CAPACITY: usize = 4096;

/// Downward acceleration on particles, m/s². Lighter than item gravity so dust
/// hangs a touch longer.
const PARTICLE_GRAVITY: f32 = -12.0;
/// Fraction of the tile a particle's UV sub-patch covers (a 4×4 texel fleck on a
/// 16px tile). Kept well inside the tile so a patch never spills past the edge.
const PATCH_FRAC: f32 = 0.25;
/// World-space size (edge length) of a particle quad, in metres.
const PARTICLE_SIZE: f32 = 0.1;
/// Fraction of lifetime over which a particle fades out at the end.
const FADE_TAIL: f32 = 0.4;

/// One terrain particle: a tiny textured quad sampling a sub-patch of `tile`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Particle {
    pub pos: Vec3,
    pub vel: Vec3,
    /// 6-bit skylight sampled when the particle spawned. Render code consumes
    /// this directly so frame mapping does not hit the world chunk map for every
    /// live fleck.
    pub skylight: u8,
    /// Block face tile this fleck is cut from.
    pub tile: Tile,
    /// Sub-tile patch origin in `[0, 1]` tile fractions (bottom-left).
    pub uv_min: [f32; 2],
    /// Sub-tile patch edge length in tile fractions.
    pub uv_size: f32,
    /// RGB tint multiplied into the fleck's atlas colour (foliage-green for a
    /// fleck cut from a grass/leaf tile, white otherwise). Classified per-fleck
    /// from [`tile`](Self::tile) so e.g. grass-top dust is green but the
    /// grass-block side/dirt dust is not.
    pub tint: [f32; 3],
    pub age: f32,
    pub lifetime: f32,
    /// World-space quad edge length, metres.
    pub size: f32,
}

impl Particle {
    /// Absolute atlas UVs for this particle's sub-patch: `(uv_min, uv_size)` in
    /// normalized atlas space, ready for a render instance. Maps the sub-tile
    /// patch into the tile's rect from [`atlas::tile_uv`].
    #[inline]
    pub fn atlas_uv(&self) -> ([f32; 2], f32) {
        let [u0, v0, u1, v1] = atlas::tile_uv(self.tile);
        let tw = u1 - u0;
        let th = v1 - v0;
        let abs_min = [u0 + self.uv_min[0] * tw, v0 + self.uv_min[1] * th];
        // Tiles are square, so the patch's atlas-space size scales by tile width.
        let abs_size = self.uv_size * tw;
        (abs_min, abs_size)
    }

    /// Normalized opacity in `[0, 1]`: full for most of the life, fading to 0 over
    /// the final [`FADE_TAIL`] fraction.
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
    /// [`FADE_TAIL`] fraction so a dying fleck visibly collapses to nothing. The
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

/// Fixed-capacity particle pool. Spawns write into a ring; ticks integrate and
/// cull in place. No allocation after construction.
pub struct ParticleSystem {
    particles: Vec<Particle>,
    /// Next write index (ring cursor) once the pool is full.
    head: usize,
    /// Monotonic counter feeding the deterministic hash for spawn variety.
    seed: u64,
}

impl ParticleSystem {
    pub fn new() -> Self {
        ParticleSystem {
            particles: Vec::with_capacity(PARTICLE_CAPACITY),
            head: 0,
            seed: 0,
        }
    }

    /// Number of currently-alive particles.
    #[inline]
    pub fn len(&self) -> usize {
        self.particles.len()
    }

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
        self.tick_with(dt, &|p| {
            Block::from_id(world.chunk_block(p.x, p.y, p.z)).is_solid()
        });
    }

    /// Pure tick behind [`tick`](Self::tick); `solid_at` reports solid cells so
    /// tests can run without a real `World`.
    fn tick_with(&mut self, dt: f32, solid_at: &impl Fn(IVec3) -> bool) {
        let mut i = 0;
        while i < self.particles.len() {
            let p = &mut self.particles[i];
            p.age += dt;
            p.vel.y += PARTICLE_GRAVITY * dt;
            let next = p.pos + p.vel * dt;
            // Stop on solid ground: if the next cell is solid, kill velocity and
            // pin to the current cell so dust settles rather than tunnelling.
            if solid_at(IVec3::new(
                next.x.floor() as i32,
                next.y.floor() as i32,
                next.z.floor() as i32,
            )) {
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
        // The ring cursor only matters while at capacity; once we've culled below
        // capacity, push() resumes appending, so reset it to avoid stale writes.
        if self.particles.len() < PARTICLE_CAPACITY {
            self.head = 0;
        }
    }

    /// Push a particle, recycling the oldest slot when the pool is full.
    #[inline]
    fn push(&mut self, p: Particle) {
        if self.particles.len() < PARTICLE_CAPACITY {
            self.particles.push(p);
        } else {
            self.particles[self.head] = p;
            self.head = (self.head + 1) % PARTICLE_CAPACITY;
        }
    }

    /// Next deterministic hash value in `[0, 1)`, advancing the internal counter.
    #[inline]
    fn rand(&mut self) -> f32 {
        self.seed = self.seed.wrapping_add(1);
        hash01(self.seed)
    }

    /// The face tile to fleck for `block`: top tile for an up/down face, side
    /// tile otherwise. `tiles()` is `[top, bottom, side]`.
    #[inline]
    fn face_tile(block: Block, face_normal: IVec3) -> Tile {
        let t = block.tiles();
        if face_normal.y > 0 {
            t[0] // top
        } else if face_normal.y < 0 {
            t[1] // bottom
        } else {
            t[2] // side
        }
    }

    /// Emit a small random sub-patch origin keeping the patch inside the tile.
    #[inline]
    fn patch_min(&mut self) -> [f32; 2] {
        let span = 1.0 - PATCH_FRAC;
        [self.rand() * span, self.rand() * span]
    }

    /// Mining face dust: 2–4 flecks spat off the mined face, drifting outward
    /// along the hit normal and falling under gravity. Lifetime 0.5–1.5 s.
    pub fn spawn_mining(&mut self, block_pos: IVec3, face_normal: IVec3, block: Block) {
        self.spawn_mining_lit(block_pos, face_normal, block, 63);
    }

    /// Same as [`spawn_mining`](Self::spawn_mining), with caller-provided
    /// render skylight on the 6-bit packed scale.
    pub fn spawn_mining_lit(
        &mut self,
        block_pos: IVec3,
        face_normal: IVec3,
        block: Block,
        skylight: u8,
    ) {
        let tile = Self::face_tile(block, face_normal);
        // Tint by the sampled face tile (a top-face grass fleck greens; the side does not).
        let tint = tile_tint(tile);
        let n = Vec3::new(
            face_normal.x as f32,
            face_normal.y as f32,
            face_normal.z as f32,
        );
        let count = 2 + (self.rand() * 3.0) as usize; // 2..=4
        let base = Vec3::new(block_pos.x as f32, block_pos.y as f32, block_pos.z as f32);
        for _ in 0..count {
            // Spawn just outside the mined face, jittered across it.
            let face_center = base + Vec3::splat(0.5) + n * 0.55;
            let jitter = Vec3::new(
                (self.rand() - 0.5) * 0.6,
                (self.rand() - 0.5) * 0.6,
                (self.rand() - 0.5) * 0.6,
            );
            let pos = face_center + jitter;
            let vel = n * (0.5 + self.rand() * 1.0)
                + Vec3::new(
                    (self.rand() - 0.5) * 1.0,
                    self.rand() * 1.5,
                    (self.rand() - 0.5) * 1.0,
                );
            let uv_min = self.patch_min();
            let lifetime = 0.5 + self.rand() * 1.0;
            self.push(Particle {
                pos,
                vel,
                skylight: skylight.min(63),
                tile,
                uv_min,
                uv_size: PATCH_FRAC,
                tint,
                age: 0.0,
                lifetime,
                size: PARTICLE_SIZE,
            });
        }
    }

    /// Break burst: 16–32 flecks erupting from the block centre in all
    /// directions. Lifetime 1–3 s. Mixes side/top tiles for visual variety.
    pub fn spawn_break_burst(&mut self, block_pos: IVec3, block: Block) {
        self.spawn_break_burst_lit(block_pos, block, 63);
    }

    /// Same as [`spawn_break_burst`](Self::spawn_break_burst), with
    /// caller-provided render skylight on the 6-bit packed scale.
    pub fn spawn_break_burst_lit(&mut self, block_pos: IVec3, block: Block, skylight: u8) {
        let tiles = block.tiles();
        let center = Vec3::new(block_pos.x as f32, block_pos.y as f32, block_pos.z as f32)
            + Vec3::splat(0.5);
        let count = 16 + (self.rand() * 16.0) as usize; // 16..=31
        for _ in 0..count {
            // Random point inside the block volume.
            let pos = center
                + Vec3::new(
                    (self.rand() - 0.5) * 0.8,
                    (self.rand() - 0.5) * 0.8,
                    (self.rand() - 0.5) * 0.8,
                );
            // Outward velocity from the centre, plus an upward bias.
            let dir = (pos - center).normalize_or_zero();
            let speed = 1.0 + self.rand() * 2.5;
            let vel = dir * speed + Vec3::new(0.0, 1.0 + self.rand() * 2.0, 0.0);
            // Pick top vs side tile per fleck.
            let tile = if self.rand() < 0.3 {
                tiles[0]
            } else {
                tiles[2]
            };
            // Tint per-fleck by the chosen tile, so a grass-top fleck greens but a
            // side/dirt fleck of the same block stays its raw atlas colour.
            let tint = tile_tint(tile);
            let uv_min = self.patch_min();
            let lifetime = 1.0 + self.rand() * 2.0;
            self.push(Particle {
                pos,
                vel,
                skylight: skylight.min(63),
                tile,
                uv_min,
                uv_size: PATCH_FRAC,
                tint,
                age: 0.0,
                lifetime,
                size: PARTICLE_SIZE,
            });
        }
    }
}

impl Default for ParticleSystem {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No solid blocks (particles never hit ground).
    fn empty(_p: IVec3) -> bool {
        false
    }

    #[test]
    fn atlas_uv_maps_into_the_tile_rect() {
        let tile = Tile::ALL[1]; // any non-trivial tile
        let [u0, v0, u1, v1] = atlas::tile_uv(tile);
        let p = Particle {
            pos: Vec3::ZERO,
            vel: Vec3::ZERO,
            skylight: 63,
            tile,
            uv_min: [0.25, 0.5],
            uv_size: 0.25,
            tint: NO_TINT,
            age: 0.0,
            lifetime: 1.0,
            size: 0.1,
        };
        let (abs_min, abs_size) = p.atlas_uv();
        let tw = u1 - u0;
        let th = v1 - v0;
        assert!((abs_min[0] - (u0 + 0.25 * tw)).abs() < 1e-6);
        assert!((abs_min[1] - (v0 + 0.5 * th)).abs() < 1e-6);
        assert!((abs_size - 0.25 * tw).abs() < 1e-6);
        // The whole patch stays inside the tile rect.
        assert!(abs_min[0] >= u0 - 1e-6 && abs_min[0] + abs_size <= u1 + 1e-6);
        assert!(abs_min[1] >= v0 - 1e-6 && abs_min[1] + abs_size <= v1 + 1e-6);
    }

    #[test]
    fn alpha_fades_at_end_of_life() {
        let mut p = Particle {
            pos: Vec3::ZERO,
            vel: Vec3::ZERO,
            skylight: 63,
            tile: Tile::ALL[0],
            uv_min: [0.0, 0.0],
            uv_size: 0.25,
            tint: NO_TINT,
            age: 0.0,
            lifetime: 1.0,
            size: 0.1,
        };
        assert_eq!(p.alpha(), 1.0, "young particle is opaque");
        p.age = 0.5;
        assert_eq!(p.alpha(), 1.0, "still inside the solid phase");
        p.age = 1.0;
        assert!(p.alpha() <= 1e-6, "fully aged is transparent");
        p.age = 0.8; // 80% through a 40% tail → 0.5
        assert!(
            (p.alpha() - 0.5).abs() < 1e-3,
            "mid-fade ~0.5, got {}",
            p.alpha()
        );
    }

    #[test]
    fn render_size_shrinks_during_fade() {
        let mut p = Particle {
            pos: Vec3::ZERO,
            vel: Vec3::ZERO,
            skylight: 63,
            tile: Tile::ALL[0],
            uv_min: [0.0, 0.0],
            uv_size: 0.25,
            tint: NO_TINT,
            age: 0.0,
            lifetime: 1.0,
            size: 0.1,
        };
        assert!(
            (p.render_size() - 0.1).abs() < 1e-6,
            "young particle is full size"
        );
        p.age = 0.8; // mid-fade, alpha ~0.5
        assert!(
            (p.render_size() - 0.05).abs() < 1e-3,
            "shrinks with the fade curve"
        );
        p.age = 1.0;
        assert!(p.render_size() <= 1e-6, "fully aged collapses to nothing");
    }

    #[test]
    fn spawn_mining_emits_two_to_four() {
        let mut sys = ParticleSystem::new();
        let before = sys.len();
        sys.spawn_mining(IVec3::new(0, 64, 0), IVec3::Y, Block::Stone);
        let n = sys.len() - before;
        assert!((2..=4).contains(&n), "mining emits 2-4 particles, got {n}");
        for p in sys.particles() {
            assert!(
                (0.5..=1.5).contains(&p.lifetime),
                "mining lifetime 0.5-1.5s"
            );
        }
    }

    #[test]
    fn spawn_break_burst_emits_a_handful() {
        let mut sys = ParticleSystem::new();
        sys.spawn_break_burst(IVec3::new(1, 2, 3), Block::Dirt);
        let n = sys.len();
        assert!(
            (16..=32).contains(&n),
            "burst emits 16-32 particles, got {n}"
        );
        for p in sys.particles() {
            assert!((1.0..=3.0).contains(&p.lifetime), "burst lifetime 1-3s");
        }
    }

    #[test]
    fn grass_top_mining_dust_is_green_but_dirt_side_is_not() {
        let grass = Biome::Plains.grass_color();
        // Mining the grass-block TOP samples GrassTop -> green flecks.
        let mut sys = ParticleSystem::new();
        sys.spawn_mining(IVec3::new(0, 64, 0), IVec3::Y, Block::Grass);
        assert!(!sys.is_empty());
        for p in sys.particles() {
            assert_eq!(p.tile, Tile::GrassTop);
            assert_eq!(p.tint, grass, "grass-top dust must be tinted green");
        }
        // Mining a grass-block SIDE samples the pre-baked GrassSide tile -> no tint.
        let mut side = ParticleSystem::new();
        side.spawn_mining(IVec3::new(0, 64, 0), IVec3::new(1, 0, 0), Block::Grass);
        for p in side.particles() {
            assert_eq!(p.tile, Tile::GrassSide);
            assert_eq!(p.tint, NO_TINT, "grass-block side dust stays untinted");
        }
        // A plain non-foliage block is never tinted on any face.
        let mut stone = ParticleSystem::new();
        stone.spawn_mining(IVec3::new(0, 64, 0), IVec3::Y, Block::Stone);
        for p in stone.particles() {
            assert_eq!(p.tint, NO_TINT, "stone dust stays untinted");
        }
    }

    #[test]
    fn leaf_burst_flecks_carry_the_foliage_tint() {
        let foliage = Biome::Plains.foliage_color();
        let mut sys = ParticleSystem::new();
        sys.spawn_break_burst(IVec3::ZERO, Block::OakLeaves);
        assert!(!sys.is_empty());
        // Leaves use the same tile on every face, so every fleck is foliage-tinted.
        for p in sys.particles() {
            assert_eq!(p.tint, foliage, "leaf fleck must carry the foliage tint");
        }
    }

    #[test]
    fn tick_ages_and_culls_dead() {
        let mut sys = ParticleSystem::new();
        sys.spawn_break_burst(IVec3::ZERO, Block::Dirt);
        assert!(!sys.is_empty());
        // Step past the maximum lifetime (3 s) so all are culled.
        for _ in 0..400 {
            sys.tick_with(0.01, &empty);
        }
        assert!(
            sys.is_empty(),
            "all particles should be culled after lifetime"
        );
    }

    #[test]
    fn respects_fixed_capacity() {
        let mut sys = ParticleSystem::new();
        // Spawn far more than capacity; the pool must never exceed PARTICLE_CAPACITY.
        for _ in 0..1000 {
            sys.spawn_break_burst(IVec3::ZERO, Block::Stone);
            assert!(
                sys.len() <= PARTICLE_CAPACITY,
                "exceeded capacity: {}",
                sys.len()
            );
        }
        assert_eq!(
            sys.len(),
            PARTICLE_CAPACITY,
            "pool should saturate at capacity"
        );
        // The backing Vec never grew past its reserved capacity (no realloc churn).
        assert_eq!(sys.particles.capacity(), PARTICLE_CAPACITY);
    }

    #[test]
    fn particles_fall_under_gravity() {
        let mut sys = ParticleSystem::new();
        sys.spawn_break_burst(IVec3::new(0, 100, 0), Block::Dirt);
        let y_before: f32 = sys.particles().iter().map(|p| p.pos.y).sum::<f32>() / sys.len() as f32;
        for _ in 0..30 {
            sys.tick_with(1.0 / 60.0, &empty);
        }
        let y_after: f32 = sys.particles().iter().map(|p| p.pos.y).sum::<f32>() / sys.len() as f32;
        assert!(
            y_after < y_before,
            "gravity should lower particles on average"
        );
    }
}
