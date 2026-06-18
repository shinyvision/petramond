//! Worldgen: 6-parameter multi-noise + jagged + surface detail + features.

use crate::block::Block;
use crate::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SEA_LEVEL};
use crate::biome::{Biome, Climate, biome_at};
use crate::mathh::smoothstep;

use noise::{
    MultiFractal, NoiseFn, OpenSimplex, Perlin, RidgedMulti, Seedable, Fbm,
};

// ---------- noise samplers ----------

pub struct WorldNoise {
    pub seed: u32,
    pub temperature: OpenSimplex,
    pub humidity: OpenSimplex,
    pub continentalness: Fbm<OpenSimplex>,
    pub erosion: OpenSimplex,
    pub weirdness: Fbm<OpenSimplex>,
    pub depth: OpenSimplex,
    pub jagged: RidgedMulti<Perlin>, // sharp ridges for peaks
    pub surface: Perlin,             // high-freq surface detail
    pub offset: Perlin,              // micro surface noise
    pub river: RidgedMulti<Perlin>,  // low-freq ridged -> rivers where ~0
}

impl WorldNoise {
    pub fn new(seed: u32) -> Self {
        let s = |salt: u32| seed.wrapping_add(salt);
        Self {
            seed,
            temperature: OpenSimplex::new(s(0x111)),
            humidity: OpenSimplex::new(s(0x222)),
            // Continentalness: smooth large-scale landmass shape.
            // 3-octave fbm, period ~768 blocks (freq ~0.0013).
            continentalness: Fbm::<OpenSimplex>::new(s(0x333))
                .set_octaves(3)
                .set_frequency(0.0013),
            // Erosion: very-low-frequency overall terrain smoothness.
            erosion: OpenSimplex::new(s(0x444)),
            // Weirdness: medium-frequency rolling variation for hills/valleys.
            weirdness: Fbm::<OpenSimplex>::new(s(0x555))
                .set_octaves(4)
                .set_frequency(0.0055),
            depth: OpenSimplex::new(s(0x666)),
            // Jagged ridges: high-freq sharp peaks. Period ~80 blocks.
            jagged: RidgedMulti::<Perlin>::default()
                .set_seed(s(0x777)).set_octaves(3).set_frequency(0.012),
            surface: Perlin::new(s(0x888)),
            offset: Perlin::new(s(0x999)),
            // River channels: low-freq ridged lines. Period ~1500 blocks.
            river: RidgedMulti::<Perlin>::default()
                .set_seed(s(0xAAA)).set_octaves(2).set_frequency(0.000_65),
        }
    }

    /// Climate sample at world (x,z) — produces 6-parameter tuple.
    pub fn climate(&self, x: i32, z: i32) -> Climate {
        let fx = x as f64;
        let fz = z as f64;
        // Temperature: slow latitude-like gradient + noise. Period ~4000.
        let t = self.temperature.get([fx * 0.000_25, fz * 0.000_25]);
        // Humidity: similar low frequency, offset.
        let h = self.humidity.get([fx * 0.000_30, fz * 0.000_30]);
        let c = self.continentalness.get([fx, fz]);
        let e = self.erosion.get([fx * 0.000_45, fz * 0.000_45]);
        let w = self.weirdness.get([fx, fz]);
        let d = self.depth.get([fx * 0.000_60, fz * 0.000_60]);
        Climate {
            temperature: t as f32,
            humidity: h as f32,
            continentalness: c as f32,
            erosion: e as f32,
            weirdness: w as f32,
            depth: d as f32,
        }
    }

    /// Surface height (top solid block Y) at world (x,z).
    /// Combines continent/erosion/weirdness base height with jagged peaks
    /// and small offset/surface detail. Returns absolute world Y.
    pub fn surface_height(&self, x: i32, z: i32) -> i32 {
        let fx = x as f64;
        let fz = z as f64;

        // Continentalness (fbm, ~[-1,1]): drives major land vs ocean shape.
        let cont = self.continentalness.get([fx, fz]);
        // Map continent to a base height. Sea level is 64; we want:
        //   cont < -0.15  -> deep ocean (~38..58)
        //   cont ≈ 0      -> coast / beaches (~62..70)
        //   cont > 0.3    -> inland plains / hills (~72..95)
        //   cont > 0.7    -> mountainous base (~95..120)
        // Using a nonlinear ramp so most terrain sits comfortably above sea.
        let cont01 = (cont * 0.5 + 0.5).clamp(0.0, 1.0);
        // base = 40 + 60 * smoothstep(0,1,cont01) gives [40..100].
        let base = 40.0 + 60.0 * smoothstep(0.0, 1.0, cont01 as f32) as f64;

        // Erosion: negative = rugged, positive = smooth/flat.
        let erosion = self.erosion.get([fx * 0.000_45, fz * 0.000_45]);
        let er_factor = (erosion * 0.5 + 0.5).clamp(0.0, 1.0); // 0 rough, 1 smooth

        // Weirdness (fbm, ~[-1,1]): medium-frequency hills and valleys.
        let weird = self.weirdness.get([fx, fz]);
        // Hills stronger where continent is high (inland) and erosion low.
        let hill_amp = (1.0 - 0.5 * er_factor) * (10.0 + 18.0 * cont01);
        let h = base + weird * hill_amp;

        // Jagged ridged noise for sharp peaks. Only meaningful inland + rugged.
        let jag = self.jagged.get([fx * 0.012, fz * 0.012]); // ~[0,1]
        let jag_amp = (1.0 - 0.85 * er_factor) * (8.0 + 22.0 * cont01);
        // Peaks gated so jagged only contributes on already-elevated terrain.
        let peak_gate = smoothstep(0.55, 0.95, jag as f32);
        let h = h + jag_amp * peak_gate as f64;

        // Surface detail (mid freq) — gentle rolling hills.
        let surf = self.surface.get([fx * 0.018, fz * 0.018]);
        let h = h + surf * 3.0 * (1.0 - 0.5 * er_factor);

        // Micro offset (high freq) — small bumps, capped.
        let off = self.offset.get([fx * 0.08, fz * 0.08]);
        let h = h + off * 1.0;

        let h = h.round() as i32;
        h.clamp(4, CHUNK_SY as i32 - 8)
    }

    /// River intensity at world (x,z): 0 = no river, 1 = carved fully.
    pub fn river_strength(&self, x: i32, z: i32) -> f32 {
        let fx = x as f64;
        let fz = z as f64;
        // Ridged noise produces ridge lines in [0,1]; rivers sit *on* ridges
        // (near 0 in RidgedMultifractal's "valley", since inverse of ridges).
        let r = self.river.get([fx * 0.001_6, fz * 0.001_6]); // [0,1]
        // We carve where the value is near 0 (between ridges), gated by an
        // additional low-freq mask so rivers don't cover the whole world.
        let mask = self.depth.get([fx * 0.000_5, fz * 0.000_5]) as f32 * 0.5 + 0.5;
        let in_channel = (1.0 - (r as f32 - 0.0).abs().min(1.0)).powi(2);
        let strength = (in_channel - 0.85).max(0.0) / 0.15; // sharp band
        strength * smoothstep(0.4, 0.9, mask)
    }
}

// ---------- biome blocks ----------

/// Pick block for the top solid surface given biome + height + river state.
pub fn surface_block(b: Biome, y: i32, river: f32) -> Block {
    if river > 0.05 && y <= SEA_LEVEL + 1 {
        return Block::Sand;
    }
    match b {
        Biome::Ocean => Block::Sand,
        Biome::Beach => Block::Sand,
        Biome::Desert => Block::Sand,
        Biome::Plains => Block::Grass,
        Biome::Forest => Block::Grass,
        Biome::BirchForest => Block::Grass,
        Biome::Savanna => Block::Grass,
        Biome::Swamp => Block::Grass,
        Biome::Taiga => Block::Grass,
        Biome::SnowyTundra => Block::Snow,
        Biome::SnowyTaiga => Block::Snow,
        Biome::Mountains => {
            if y > 95 { Block::Snow }
            else if y > 78 { Block::Stone }
            else { Block::Grass }
        }
        Biome::SnowyPeaks => Block::Snow,
        Biome::River => Block::Sand,
    }
}

pub fn subsurface_block(b: Biome) -> Block {
    match b {
        Biome::Desert => Block::Sand,
        Biome::Beach => Block::Sand,
        Biome::Mountains if true => Block::Stone, // below surface in mtns
        Biome::SnowyPeaks => Block::Stone,
        _ => Block::Dirt,
    }
}

/// Build column block stack for (x,z) into chunk buffer.
pub fn build_column(noise: &WorldNoise, chunk: &mut Chunk, x: usize, z: usize) {
    let (wx, wz) = {
        let (ox, oz) = chunk.chunk_origin_world();
        (ox + x as i32, oz + z as i32)
    };
    let surf = noise.surface_height(wx, wz);
    let climate = noise.climate(wx, wz);
    let biome = biome_at(climate, surf);
    let river = noise.river_strength(wx, wz);

    let top = surface_block(biome, surf, river);
    let sub = subsurface_block(biome);

    chunk.set_biome(x, z, biome.id());

    let carve = river > 0.05;
    let river_bed_y = (SEA_LEVEL - 2).max(surf - 4);

    for y in 0..CHUNK_SY {
        let y = y as i32;
        let b = if y > surf {
            if y <= SEA_LEVEL { Block::Water } else { Block::Air }
        } else if carve && y >= river_bed_y && y <= SEA_LEVEL {
            if y <= SEA_LEVEL { Block::Water } else { Block::Air }
        } else if carve && y == river_bed_y - 1 {
            sub
        } else if y == surf {
            top
        } else if y > surf - 5 {
            sub
        } else {
            Block::Stone
        };
        if b != Block::Air {
            chunk.set_block_raw(x, y as usize, z, b.id());
        }
    }
}

/// Generate terrain + features for a chunk. Caller passes seed.
pub fn generate_chunk(seed: u32, cx: i32, cz: i32) -> Chunk {
    let mut chunk = Chunk::new(cx, cz);
    let noise = WorldNoise::new(seed);

    // Terrain columns.
    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            build_column(&noise, &mut chunk, x, z);
        }
    }

    // Trees & features layered on top via deterministic RNG seeded per chunk.
    let mut rng = crate::gen::rng::FeatureRng::new(seed, cx, cz);
    crate::gen::features::place_features(&mut chunk, &noise, &mut rng);

    chunk.dirty = true;
    chunk
}

pub mod rng {
    /// xorshift64 RNG seeded deterministically from world seed + chunk pos.
    pub struct FeatureRng { state: u64 }
    impl FeatureRng {
        pub fn new(seed: u32, cx: i32, cz: i32) -> Self {
            let mut s = seed as u64
                ^ ((cx as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15))
                ^ ((cz as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F));
            if s == 0 { s = 0xDEAD_BEEF; }
            Self { state: s }
        }
        pub fn next_u64(&mut self) -> u64 {
            let mut x = self.state;
            x ^= x << 13; x ^= x >> 7; x ^= x << 17;
            self.state = x; x
        }
        pub fn next_f32(&mut self) -> f32 {
            (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
        }
        pub fn next_i32(&mut self, lo: i32, hi: i32) -> i32 {
            lo + (self.next_u64() % (hi - lo + 1).max(1) as u64) as i32
        }
        pub fn chance(&mut self, p: f32) -> bool { self.next_f32() < p }
    }
}

pub mod features {
    use super::*;
    use crate::chunk::{Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SEA_LEVEL};
    use crate::gen::rng::FeatureRng;
    use crate::gen::WorldNoise;
    use crate::gen::trees;

    pub fn place_features(chunk: &mut Chunk, noise: &WorldNoise, rng: &mut FeatureRng) {
        // Trees: per-column probability, biome-dependent.
        let (ox, oz) = chunk.chunk_origin_world();
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                let wx = ox + x as i32;
                let wz = oz + z as i32;
                if x == 0 || z == 0 || x == CHUNK_SX - 1 || z == CHUNK_SZ - 1 {
                    // Avoid trees at chunk edges to minimise cross-chunk
                    // collisions (true impl would query neighbours).
                    continue;
                }
                let surf = noise.surface_height(wx, wz);
                if surf <= SEA_LEVEL { continue; }
                let climate = noise.climate(wx, wz);
                let biome = biome_at(climate, surf);
                let p = tree_probability(biome);
                if !rng.chance(p) { continue; }

                let variant = pick_oak_variant(rng, biome);
                place_oak(chunk, x, surf, z, variant, rng);
            }
        }
    }

    fn tree_probability(b: Biome) -> f32 {
        match b {
            Biome::Forest => 0.06,
            Biome::BirchForest => 0.04,
            Biome::Plains => 0.012,
            Biome::Savanna => 0.015,
            Biome::Swamp => 0.014,
            Biome::Taiga => 0.010,
            Biome::SnowyTaiga => 0.010,
            Biome::SnowyTundra => 0.002,
            _ => 0.0,
        }
    }

    fn pick_oak_variant(rng: &mut FeatureRng, b: Biome) -> trees::OakVariant {
        use trees::OakVariant::*;
        // Distribution biased by biome.
        match b {
            Biome::Forest => match rng.next_i32(0, 99) {
                0..=4 => OakBig,
                5..=44 => Oak2,
                45..=74 => Oak3,
                _ => Oak1,
            },
            Biome::Plains | Biome::Savanna => match rng.next_i32(0, 99) {
                0..=2 => OakBig,
                3..=72 => Oak1,
                _ => Oak4,
            },
            Biome::Swamp => match rng.next_i32(0, 99) {
                0..=9 => OakBig,
                _ => Oak4,
            },
            _ => match rng.next_i32(0, 99) {
                0..=2 => OakBig,
                _ => Oak1,
            },
        }
    }

    fn place_oak(
        chunk: &mut Chunk, x: usize, y: i32, z: usize,
        variant: trees::OakVariant, rng: &mut FeatureRng,
    ) {
        if y < 1 || y + 12 >= CHUNK_SY as i32 { return; }
        trees::place(chunk, x, y, z, variant, rng);
    }
}

pub mod trees {
    //! Five oak variants.
    //!
    //! oak_1: classic 4-5 tall straight, leaf blob.
    //! oak_2: taller 6-7 with slight lean.
    //! oak_3: 4-tall with canopy wider/offset.
    //! oak_4: swamp-style: 5 tall, leaves drooping one block lower on sides.
    //! oak_big: procedurally generated: 2x2 trunk, diagonal log branches,
    //! layered leaves around canopy corners.

    use super::rng::FeatureRng;
    use crate::block::Block;
    use crate::chunk::{Chunk, CHUNK_SY};

    #[derive(Copy, Clone, Debug)]
    pub enum OakVariant { Oak1, Oak2, Oak3, Oak4, OakBig }

    pub fn place(
        chunk: &mut Chunk, x: usize, y: i32, z: usize,
        v: OakVariant, rng: &mut FeatureRng,
    ) {
        match v {
            OakVariant::Oak1 => oak_simple(chunk, x, y, z, 4 + rng.next_i32(0,1), 0, 0, rng),
            OakVariant::Oak2 => oak_simple(chunk, x, y, z, 6 + rng.next_i32(0,1), rng.next_i32(-1,1), rng.next_i32(-1,1), rng),
            OakVariant::Oak3 => oak_canopy_offset(chunk, x, y, z, 4, rng),
            OakVariant::Oak4 => oak_swamp(chunk, x, y, z, 5 + rng.next_i32(0,1), rng),
            OakVariant::OakBig => oak_big(chunk, x, y, z, rng),
        }
    }

    fn log_at(chunk: &mut Chunk, x: i32, y: i32, z: i32) {
        if in_bounds(x, y, z) {
            chunk.set_block_raw(x as usize, y as usize, z as usize, Block::OakLog.id());
        }
    }
    fn leaf_at(chunk: &mut Chunk, x: i32, y: i32, z: i32) {
        if in_bounds(x, y, z) {
            // Only overwrite air/water.
            let b = chunk.block_raw(x as usize, y as usize, z as usize);
            if b == Block::Air.id() || b == Block::Water.id() {
                chunk.set_block_raw(x as usize, y as usize, z as usize, Block::OakLeaves.id());
            }
        }
    }
    fn in_bounds(x: i32, y: i32, z: i32) -> bool {
        x >= 0 && x < 16 && z >= 0 && z < 16 && y >= 0 && y < CHUNK_SY as i32
    }

    /// Classic straight oak with small lean offset.
    fn oak_simple(
        chunk: &mut Chunk, x: usize, y: i32, z: usize,
        height: i32, dx: i32, dz: i32, _rng: &mut FeatureRng,
    ) {
        let mut cx = x as i32;
        let mut cz = z as i32;
        for i in 0..height {
            log_at(chunk, cx, y + i, cz);
            // Apply lean by shifting mid-way up.
            if i == height / 2 { cx += dx; cz += dz; }
        }
        // Leaf blob centered around last 2 logs.
        let top = y + height - 1;
        leaf_blob(chunk, cx, top, cz, 2, false);
    }

    /// Short oak with a single offset canopy corner (oak_3).
    fn oak_canopy_offset(
        chunk: &mut Chunk, x: usize, y: i32, z: usize,
        height: i32, rng: &mut FeatureRng,
    ) {
        let dx = rng.next_i32(-1, 1);
        let dz = rng.next_i32(-1, 1);
        for i in 0..height {
            log_at(chunk, x as i32, y + i, z as i32);
        }
        let top = y + height - 1;
        // Wider asymmetric blob.
        for ly in -1i32..=2 {
            let r: i32 = if ly <= 0 { 2 } else { 1 };
            for lx in -r..=r {
                for lz in -r..=r {
                    if lx == 0 && lz == 0 && ly < 2 { continue; }
                    if (lx.abs() == r && lz.abs() == r) && rng.chance(0.5) { continue; }
                    leaf_at(chunk, x as i32 + lx + dx * (ly / 2), top + ly, z as i32 + lz + dz * (ly / 2));
                }
            }
        }
    }

    /// Swamp oak: droopy leaves (one block lower on sides).
    fn oak_swamp(
        chunk: &mut Chunk, x: usize, y: i32, z: usize,
        height: i32, rng: &mut FeatureRng,
    ) {
        for i in 0..height {
            log_at(chunk, x as i32, y + i, z as i32);
        }
        let top = y + height - 1;
        // Top small cap.
        for lx in -1i32..=1 {
            for lz in -1i32..=1 {
                if lx == 0 && lz == 0 { continue; }
                if rng.chance(0.3) { continue; }
                leaf_at(chunk, x as i32 + lx, top + 1, z as i32 + lz);
            }
        }
        // Droopy lower layer.
        for lx in -2i32..=2 {
            for lz in -2i32..=2 {
                if lx.abs() == 2 && lz.abs() == 2 { continue; }
                if rng.chance(0.6) { continue; }
                leaf_at(chunk, x as i32 + lx, top - 1, z as i32 + lz);
            }
        }
        leaf_at(chunk, x as i32, top + 1, z as i32);
    }

    /// Big oak: 2x2 trunk, procedural log branches, layered leaves.
    fn oak_big(chunk: &mut Chunk, x: usize, y: i32, z: usize, rng: &mut FeatureRng) {
        // Reserve 2x2 footprint. Caller already skipped edges.
        if x + 1 >= 16 || z + 1 >= 16 { return; }
        let height = 8 + rng.next_i32(0, 4); // 8..12
        // Trunk: 2x2 column. Logs up to height-2, then single center for crown.
        let base = y;
        for i in 0..height {
            let h = base + i;
            log_at(chunk, x as i32,     h, z as i32);
            log_at(chunk, x as i32 + 1, h, z as i32);
            log_at(chunk, x as i32,     h, z as i32 + 1);
            log_at(chunk, x as i32 + 1, h, z as i32 + 1);
        }
        // Branches: starting at ~70% height, walk 2-3 logs diagonally out/up.
        let crown_base = base + (height * 7 / 10);
        let branch_count = rng.next_i32(2, 4);
        for _ in 0..branch_count {
            let sx = x as i32 + rng.next_i32(0, 1);
            let sz = z as i32 + rng.next_i32(0, 1);
            let sy = crown_base + rng.next_i32(0, 2);
            let (bdx, bdz) = match rng.next_i32(0, 7) {
                0 => (-1,  0), 1 => ( 1,  0), 2 => ( 0, -1), 3 => ( 0,  1),
                4 => (-1, -1), 5 => (-1,  1), 6 => ( 1, -1), _ => ( 1,  1),
            };
            let len = rng.next_i32(2, 4);
            let (mut bx, mut by, mut bz) = (sx, sy, sz);
            for _ in 0..len {
                bx += bdx; by += 1; bz += bdz;
                if in_bounds(bx, by, bz) {
                    // Replace leaves if needed.
                    let cur = chunk.block_raw(bx as usize, by as usize, bz as usize);
                    if cur == Block::Air.id() || cur == Block::OakLeaves.id() || cur == Block::Water.id() {
                        chunk.set_block_raw(bx as usize, by as usize, bz as usize, Block::OakLog.id());
                    }
                }
            }
            // Leaf cluster at branch tip.
            leaf_blob(chunk, bx, by, bz, 2, false);
        }
        // Crown: layered leaves around top of trunk (2x2 center).
        let top = base + height - 1;
        let cx = x as i32 + 1;
        let cz = z as i32 + 1;
        // Layer 0 (just below top): radius 2.
        for lx in -2i32..=2 {
            for lz in -2i32..=2 {
                if lx.abs() == 2 && lz.abs() == 2 { continue; }
                leaf_at(chunk, cx + lx, top - 1, cz + lz);
            }
        }
        // Layer 1 (top): radius 1, plus corners randomly.
        for lx in -1i32..=1 {
            for lz in -1i32..=1 {
                if lx == 0 && lz == 0 { leaf_at(chunk, cx, top + 1, cz); continue; }
                if (lx.abs() == 1 && lz.abs() == 1) && rng.chance(0.5) { continue; }
                leaf_at(chunk, cx + lx, top, cz + lz);
            }
        }
        // Layer 2 (above): small cap.
        for lx in -1i32..=1 {
            for lz in -1i32..=1 {
                if lx.abs() == 1 && lz.abs() == 1 { continue; }
                if rng.chance(0.4) { continue; }
                leaf_at(chunk, cx + lx, top + 1, cz + lz);
            }
        }
    }

    /// Spherical-ish leaf blob centered at (x,y,z).
    fn leaf_blob(
        chunk: &mut Chunk, cx: i32, cy: i32, cz: i32,
        radius: i32, allow_overwrite: bool,
    ) {
        let r = radius;
        for ly in -r..=r {
            for lx in -r..=r {
                for lz in -r..=r {
                    let d2 = lx*lx + ly*ly + lz*lz;
                    if d2 > r*r + 1 { continue; }
                    if d2 > r*r - 1 && (lx.abs() == r || lz.abs() == r || ly.abs() == r) {
                        continue;
                    }
                    if !in_bounds(cx + lx, cy + ly, cz + lz) { continue; }
                    let bx = (cx + lx) as usize;
                    let by = (cy + ly) as usize;
                    let bz = (cz + lz) as usize;
                    if allow_overwrite {
                        chunk.set_block_raw(bx, by, bz, Block::OakLeaves.id());
                    } else {
                        let cur = chunk.block_raw(bx, by, bz);
                        if cur == Block::Air.id() || cur == Block::Water.id() {
                            chunk.set_block_raw(bx, by, bz, Block::OakLeaves.id());
                        }
                    }
                }
            }
        }
    }
}