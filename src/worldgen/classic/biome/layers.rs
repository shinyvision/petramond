//! Per-layer transforms of the biome cascade + the layer-stack framework.
//!
//! A [`Layer`] fills a `w × h` grid (row-major, `out[j*w + i]` for cell
//! `(x0+i, z0+j)` in this layer's own coordinate scale). Layers compose by holding
//! their parent and recursively requesting the parent area they need. Each layer
//! is ported from the authoritative reference and verified bit-exact against the
//! reference's per-layer output before the next is added.
//!
//! Ported so far: continent (land/ocean seed), zoom (normal + fuzzy).

use super::super::layer_rng::{
    cell_seed, first_int, first_is_zero, layer_salt, start_seed, step, LayerRng,
};
use super::ids::*;

/// 32-bit zoom RNG constants (the zoom layer uses its own 32-bit generator).
const ZMUL: u32 = 1284865837;
const ZADD: u32 = 4150755663;

/// A biome-cascade layer: fills a grid in its own coordinate scale.
pub trait Layer: Send + Sync {
    fn gen(&self, x: i64, z: i64, w: usize, h: usize) -> Vec<i32>;
}

/// Continent layer (scale 4096): each cell is land(1) w.p. 1/10 else ocean(0);
/// the cell containing world origin `(0,0)` is forced to land.
pub struct Continent {
    start_seed: i64,
}

impl Continent {
    pub fn new(world_seed: i64) -> Self {
        Self {
            start_seed: start_seed(world_seed, layer_salt(1)),
        }
    }
}

impl Layer for Continent {
    fn gen(&self, x0: i64, z0: i64, w: usize, h: usize) -> Vec<i32> {
        let mut out = vec![0i32; w * h];
        for j in 0..h as i64 {
            for i in 0..w as i64 {
                let cs = cell_seed(self.start_seed, x0 + i, z0 + j);
                out[(j as usize) * w + i as usize] = i32::from(first_is_zero(cs, 10));
            }
        }
        if x0 > -(w as i64) && x0 <= 0 && z0 > -(h as i64) && z0 <= 0 {
            out[(-z0 * w as i64 - x0) as usize] = 1;
        }
        out
    }
}

/// Reservoir-free 4-corner resolve for the zoom's south-east child: pick the
/// corner value that matches the most neighbours, breaking a genuine 4-way tie
/// with one 32-bit draw (`cs` is the state after the north-east child's advance).
#[inline]
fn select4(cs: u32, st: u32, v00: i32, v01: i32, v10: i32, v11: i32) -> i32 {
    let cv00 = (v00 == v10) as i32 + (v00 == v01) as i32 + (v00 == v11) as i32;
    let cv10 = (v10 == v01) as i32 + (v10 == v11) as i32;
    let cv01 = (v01 == v11) as i32;
    if cv00 > cv10 && cv00 > cv01 {
        v00
    } else if cv10 > cv00 {
        v10
    } else if cv01 > cv00 {
        v01
    } else {
        let r = (zoom_step(cs, st) >> 24) & 3;
        [v00, v10, v01, v11][r as usize]
    }
}

#[inline]
fn zoom_step(cs: u32, salt: u32) -> u32 {
    cs.wrapping_mul(cs.wrapping_mul(ZMUL).wrapping_add(ZADD))
        .wrapping_add(salt)
}

/// Zoom ×2 layer: doubles resolution. `fuzzy` always picks a random corner for
/// the centre child (no equality smoothing); non-fuzzy resolves ties by majority
/// ([`select4`]). Uses the 32-bit zoom RNG seeded from the low 32 bits of the
/// layer's start seed/salt.
pub struct Zoom {
    start_salt: u32,
    start_seed: u32,
    fuzzy: bool,
    parent: Box<dyn Layer>,
}

impl Zoom {
    pub fn new(world_seed: i64, salt: i64, fuzzy: bool, parent: Box<dyn Layer>) -> Self {
        let r = LayerRng::new(world_seed, salt);
        Self {
            start_salt: r.start_salt() as u32,
            start_seed: r.start_seed() as u32,
            fuzzy,
            parent,
        }
    }
}

impl Layer for Zoom {
    fn gen(&self, x: i64, z: i64, w: usize, h: usize) -> Vec<i32> {
        let p_x = x >> 1;
        let p_z = z >> 1;
        let p_w = ((x + w as i64) >> 1) - p_x + 1;
        let p_h = ((z + h as i64) >> 1) - p_z + 1;
        let pw = p_w as usize;
        let ph = p_h as usize;
        // Request a +1 border so the (i+1, j+1) corner reads stay in-bounds with
        // real parent cells; the extra column/row only feeds trimmed buffer cells.
        let gw = pw + 1;
        let parent = self.parent.gen(p_x, p_z, gw, ph + 1);

        let new_w = pw * 2;
        let new_h = ph * 2;
        let mut buf = vec![0i32; new_w * new_h];
        let (ss, st) = (self.start_seed, self.start_salt);

        for j in 0..ph {
            for i in 0..pw {
                let v00 = parent[i + j * gw];
                let v10 = parent[(i + 1) + j * gw];
                let v01 = parent[i + (j + 1) * gw];
                let v11 = parent[(i + 1) + (j + 1) * gw];
                let base = (2 * j) * new_w + 2 * i;

                if v00 == v01 && v00 == v10 && v00 == v11 {
                    buf[base] = v00;
                    buf[base + 1] = v00;
                    buf[base + new_w] = v00;
                    buf[base + new_w + 1] = v00;
                    continue;
                }

                let chunk_x = ((i as i64 + p_x) * 2) as i32 as u32;
                let chunk_z = ((j as i64 + p_z) * 2) as i32 as u32;
                // Seed mix: cs = ss; cs += x; cs = cs*(cs*ZMUL+ZADD); cs += z; … —
                // the coordinate post-add sits between multiplies, so spell it out.
                let mut cs = ss;
                cs = cs.wrapping_add(chunk_x);
                cs = cs.wrapping_mul(cs.wrapping_mul(ZMUL).wrapping_add(ZADD));
                cs = cs.wrapping_add(chunk_z);
                cs = cs.wrapping_mul(cs.wrapping_mul(ZMUL).wrapping_add(ZADD));
                cs = cs.wrapping_add(chunk_x);
                cs = cs.wrapping_mul(cs.wrapping_mul(ZMUL).wrapping_add(ZADD));
                cs = cs.wrapping_add(chunk_z);

                buf[base] = v00; // NW
                buf[base + new_w] = if (cs >> 24) & 1 != 0 { v01 } else { v00 }; // SW

                cs = zoom_step(cs, st);
                buf[base + 1] = if (cs >> 24) & 1 != 0 { v10 } else { v00 }; // NE

                buf[base + new_w + 1] = if self.fuzzy {
                    cs = zoom_step(cs, st);
                    [v00, v10, v01, v11][((cs >> 24) & 3) as usize]
                } else {
                    select4(cs, st, v00, v01, v10, v11)
                };
            }
        }

        let off_x = (x & 1) as usize;
        let off_z = (z & 1) as usize;
        let mut out = vec![0i32; w * h];
        for j in 0..h {
            for i in 0..w {
                out[j * w + i] = buf[(j + off_z) * new_w + off_x + i];
            }
        }
        out
    }
}

/// Land / add-island layer: grows and erodes coastlines. For an ocean centre with
/// at least one non-ocean diagonal corner, a reservoir sample over the corners
/// picks the new value (then a 2/3 chance to stay ocean); for a land centre
/// touching ocean, a 1/5 chance erodes it to ocean. Reads a 3×3 diagonal
/// neighbourhood (corners two cells out); uses the 64-bit generator.
pub struct Land {
    start_salt: i64,
    start_seed: i64,
    parent: Box<dyn Layer>,
}

impl Land {
    pub fn new(world_seed: i64, salt: i64, parent: Box<dyn Layer>) -> Self {
        let r = LayerRng::new(world_seed, salt);
        Self {
            start_salt: r.start_salt(),
            start_seed: r.start_seed(),
            parent,
        }
    }
}

impl Layer for Land {
    fn gen(&self, x: i64, z: i64, w: usize, h: usize) -> Vec<i32> {
        let pw = w + 2;
        let parent = self.parent.gen(x - 1, z - 1, pw, h + 2);
        let mut out = vec![0i32; w * h];
        let (ss, st) = (self.start_seed, self.start_salt);
        for j in 0..h {
            for i in 0..w {
                let v00 = parent[i + j * pw]; // NW corner (two cells out)
                let v20 = parent[(i + 2) + j * pw]; // NE
                let v02 = parent[i + (j + 2) * pw]; // SW
                let v22 = parent[(i + 2) + (j + 2) * pw]; // SE
                let v11 = parent[(i + 1) + (j + 1) * pw]; // centre
                let mut v = v11;

                if v11 == 0 {
                    if v00 != 0 || v20 != 0 || v02 != 0 || v22 != 0 {
                        let mut cs = cell_seed(ss, x + i as i64, z + j as i64);
                        let mut inc = 0;
                        v = 1;
                        if v00 != 0 {
                            inc += 1;
                            v = v00;
                            cs = step(cs, st);
                        }
                        if v20 != 0 {
                            inc += 1;
                            if inc == 1 || first_is_zero(cs, 2) {
                                v = v20;
                            }
                            cs = step(cs, st);
                        }
                        if v02 != 0 {
                            inc += 1;
                            match inc {
                                1 => v = v02,
                                2 => {
                                    if first_is_zero(cs, 2) {
                                        v = v02;
                                    }
                                }
                                _ => {
                                    if first_is_zero(cs, 3) {
                                        v = v02;
                                    }
                                }
                            }
                            cs = step(cs, st);
                        }
                        if v22 != 0 {
                            inc += 1;
                            match inc {
                                1 => v = v22,
                                2 => {
                                    if first_is_zero(cs, 2) {
                                        v = v22;
                                    }
                                }
                                3 => {
                                    if first_is_zero(cs, 3) {
                                        v = v22;
                                    }
                                }
                                _ => {
                                    if first_is_zero(cs, 4) {
                                        v = v22;
                                    }
                                }
                            }
                            cs = step(cs, st);
                        }
                        if v != 4 && !first_is_zero(cs, 3) {
                            v = 0;
                        }
                    }
                } else if v11 != 4 {
                    // Land centre touching ocean: small chance to erode.
                    if v00 == 0 || v20 == 0 || v02 == 0 || v22 == 0 {
                        let cs = cell_seed(ss, x + i as i64, z + j as i64);
                        if first_is_zero(cs, 5) {
                            v = 0;
                        }
                    }
                }
                out[i + j * w] = v;
            }
        }
        out
    }
}

/// Fetch a parent grid with a 1-cell border (`x-1, z-1, w+2, h+2`). Returns the
/// grid and its width `w+2`. Shared by the same-scale neighbourhood layers.
fn bordered_parent(parent: &dyn Layer, x: i64, z: i64, w: usize, h: usize) -> (Vec<i32>, usize) {
    let pw = w + 2;
    (parent.gen(x - 1, z - 1, pw, h + 2), pw)
}

/// A one-parent layer built from a salt (start seed/salt) — the shape shared by
/// the neighbourhood transforms below.
macro_rules! salted_layer {
    ($name:ident) => {
        pub struct $name {
            start_salt: i64,
            start_seed: i64,
            parent: Box<dyn Layer>,
        }
        impl $name {
            pub fn new(world_seed: i64, salt: i64, parent: Box<dyn Layer>) -> Self {
                let r = LayerRng::new(world_seed, salt);
                Self {
                    start_salt: r.start_salt(),
                    start_seed: r.start_seed(),
                    parent,
                }
            }
        }
    };
}

salted_layer!(RemoveOcean);
impl Layer for RemoveOcean {
    fn gen(&self, x: i64, z: i64, w: usize, h: usize) -> Vec<i32> {
        let _ = self.start_salt;
        let (p, pw) = bordered_parent(&*self.parent, x, z, w, h);
        let mut out = vec![0i32; w * h];
        for j in 0..h {
            for i in 0..w {
                let v11 = p[(i + 1) + (j + 1) * pw];
                let mut v = v11;
                if v11 == OCEAN
                    && p[(i + 1) + j * pw] == OCEAN
                    && p[(i + 2) + (j + 1) * pw] == OCEAN
                    && p[i + (j + 1) * pw] == OCEAN
                    && p[(i + 1) + (j + 2) * pw] == OCEAN
                {
                    let cs = cell_seed(self.start_seed, x + i as i64, z + j as i64);
                    if first_is_zero(cs, 2) {
                        v = 1;
                    }
                }
                out[i + j * w] = v;
            }
        }
        out
    }
}

salted_layer!(Snow);
impl Layer for Snow {
    fn gen(&self, x: i64, z: i64, w: usize, h: usize) -> Vec<i32> {
        let _ = self.start_salt;
        let (p, pw) = bordered_parent(&*self.parent, x, z, w, h);
        let mut out = vec![0i32; w * h];
        for j in 0..h {
            for i in 0..w {
                let v11 = p[(i + 1) + (j + 1) * pw];
                out[i + j * w] = if !is_shallow_ocean(v11) {
                    let cs = cell_seed(self.start_seed, x + i as i64, z + j as i64);
                    match first_int(cs, 6) {
                        0 => T_FREEZING,
                        1 => T_COLD,
                        _ => T_WARM,
                    }
                } else {
                    v11
                };
            }
        }
        out
    }
}

/// Cool↔warm edge: a warm cell orthogonally adjacent to a cold/freezing cell
/// becomes temperate (lush). No RNG.
pub struct Cool {
    parent: Box<dyn Layer>,
}
impl Cool {
    pub fn new(parent: Box<dyn Layer>) -> Self {
        Self { parent }
    }
}
impl Layer for Cool {
    fn gen(&self, x: i64, z: i64, w: usize, h: usize) -> Vec<i32> {
        let (p, pw) = bordered_parent(&*self.parent, x, z, w, h);
        let mut out = vec![0i32; w * h];
        for j in 0..h {
            for i in 0..w {
                let mut v11 = p[(i + 1) + (j + 1) * pw];
                if v11 == T_WARM {
                    let n = [
                        p[(i + 1) + j * pw],
                        p[(i + 2) + (j + 1) * pw],
                        p[i + (j + 1) * pw],
                        p[(i + 1) + (j + 2) * pw],
                    ];
                    if n.contains(&T_COLD) || n.contains(&T_FREEZING) {
                        v11 = T_LUSH;
                    }
                }
                out[i + j * w] = v11;
            }
        }
        out
    }
}

/// Heat↔ice edge: a freezing cell orthogonally adjacent to a warm/lush cell
/// becomes cold. No RNG.
pub struct Heat {
    parent: Box<dyn Layer>,
}
impl Heat {
    pub fn new(parent: Box<dyn Layer>) -> Self {
        Self { parent }
    }
}
impl Layer for Heat {
    fn gen(&self, x: i64, z: i64, w: usize, h: usize) -> Vec<i32> {
        let (p, pw) = bordered_parent(&*self.parent, x, z, w, h);
        let mut out = vec![0i32; w * h];
        for j in 0..h {
            for i in 0..w {
                let mut v11 = p[(i + 1) + (j + 1) * pw];
                if v11 == T_FREEZING {
                    let n = [
                        p[(i + 1) + j * pw],
                        p[(i + 2) + (j + 1) * pw],
                        p[i + (j + 1) * pw],
                        p[(i + 1) + (j + 2) * pw],
                    ];
                    if n.contains(&T_WARM) || n.contains(&T_LUSH) {
                        v11 = T_COLD;
                    }
                }
                out[i + j * w] = v11;
            }
        }
        out
    }
}

// Mutation marker (special): a non-ocean cell gets a 1-in-13 chance to set a
// mutation value into bits 8..12. Same-scale (1:1).
salted_layer!(Special);
impl Layer for Special {
    fn gen(&self, x: i64, z: i64, w: usize, h: usize) -> Vec<i32> {
        let mut out = self.parent.gen(x, z, w, h);
        for j in 0..h {
            for i in 0..w {
                let idx = i + j * w;
                let mut v = out[idx];
                if v != T_OCEANIC {
                    let mut cs = cell_seed(self.start_seed, x + i as i64, z + j as i64);
                    if first_is_zero(cs, 13) {
                        cs = step(cs, self.start_salt);
                        v |= ((1 + first_int(cs, 15)) << 8) & MUTATION_MASK;
                        out[idx] = v;
                    }
                }
            }
        }
        out
    }
}

// Add mushroom island: an ocean cell with all four DIAGONAL corners ocean has a
// 1-in-100 chance to become mushroom fields.
salted_layer!(Mushroom);
impl Layer for Mushroom {
    fn gen(&self, x: i64, z: i64, w: usize, h: usize) -> Vec<i32> {
        let _ = self.start_salt;
        let (p, pw) = bordered_parent(&*self.parent, x, z, w, h);
        let mut out = vec![0i32; w * h];
        for j in 0..h {
            for i in 0..w {
                let v11 = p[(i + 1) + (j + 1) * pw];
                let mut v = v11;
                if v11 == OCEAN
                    && p[i + j * pw] == OCEAN
                    && p[(i + 2) + j * pw] == OCEAN
                    && p[i + (j + 2) * pw] == OCEAN
                    && p[(i + 2) + (j + 2) * pw] == OCEAN
                {
                    let cs = cell_seed(self.start_seed, x + i as i64, z + j as i64);
                    if first_is_zero(cs, 100) {
                        v = MUSHROOM_FIELDS;
                    }
                }
                out[i + j * w] = v;
            }
        }
        out
    }
}

/// Deep ocean: a shallow-ocean cell with all four orthogonal neighbours shallow
/// ocean becomes deep ocean. No RNG.
pub struct DeepOcean {
    parent: Box<dyn Layer>,
}
impl DeepOcean {
    pub fn new(parent: Box<dyn Layer>) -> Self {
        Self { parent }
    }
}
impl Layer for DeepOcean {
    fn gen(&self, x: i64, z: i64, w: usize, h: usize) -> Vec<i32> {
        let (p, pw) = bordered_parent(&*self.parent, x, z, w, h);
        let mut out = vec![0i32; w * h];
        for j in 0..h {
            for i in 0..w {
                let v11 = p[(i + 1) + (j + 1) * pw];
                let mut v = v11;
                if is_shallow_ocean(v11) {
                    let oceans = is_shallow_ocean(p[(i + 1) + j * pw]) as i32
                        + is_shallow_ocean(p[(i + 2) + (j + 1) * pw]) as i32
                        + is_shallow_ocean(p[i + (j + 1) * pw]) as i32
                        + is_shallow_ocean(p[(i + 1) + (j + 2) * pw]) as i32;
                    if oceans >= 4 {
                        v = DEEP_OCEAN;
                    }
                }
                out[i + j * w] = v;
            }
        }
        out
    }
}

// Biome assignment: climate tag (+ optional mutation bit) → a concrete biome.
// Same-scale (1:1).
salted_layer!(Biome);
impl Layer for Biome {
    fn gen(&self, x: i64, z: i64, w: usize, h: usize) -> Vec<i32> {
        let _ = self.start_salt;
        let src = self.parent.gen(x, z, w, h);
        let mut out = vec![0i32; w * h];
        for j in 0..h {
            for i in 0..w {
                let idx = i + j * w;
                let raw = src[idx];
                let has_high = raw & MUTATION_MASK;
                let id = raw & !MUTATION_MASK;
                out[idx] = if is_oceanic(id) || id == MUSHROOM_FIELDS {
                    id
                } else {
                    let cs = cell_seed(self.start_seed, x + i as i64, z + j as i64);
                    match id {
                        T_WARM => {
                            if has_high != 0 {
                                if first_is_zero(cs, 3) {
                                    BADLANDS_PLATEAU
                                } else {
                                    WOODED_BADLANDS_PLATEAU
                                }
                            } else {
                                WARM_BIOMES[first_int(cs, 6) as usize]
                            }
                        }
                        T_LUSH => {
                            if has_high != 0 {
                                JUNGLE
                            } else {
                                LUSH_BIOMES[first_int(cs, 6) as usize]
                            }
                        }
                        T_COLD => {
                            if has_high != 0 {
                                GIANT_TREE_TAIGA
                            } else {
                                COLD_BIOMES[first_int(cs, 4) as usize]
                            }
                        }
                        T_FREEZING => SNOW_BIOMES[first_int(cs, 4) as usize],
                        _ => MUSHROOM_FIELDS,
                    }
                };
            }
        }
        out
    }
}

/// Edge replacement: if `centre == base`, keep it when all four orthogonal
/// neighbours are the same family, else replace with `edge`. Returns `None` when
/// the rule does not apply (centre is not `base`).
#[inline]
fn replace_edge(n: [i32; 4], centre: i32, base: i32, edge: i32) -> Option<i32> {
    if centre != base {
        return None;
    }
    if n.iter().all(|&v| are_similar(v, base)) {
        Some(centre)
    } else {
        Some(edge)
    }
}

// Biome edge: badlands-plateau / giant-tree-taiga edges, plus desert<->snow and
// swamp<->(desert/snow/jungle) special cases. Reads the four orthogonal neighbours.
salted_layer!(BiomeEdge);
impl Layer for BiomeEdge {
    fn gen(&self, x: i64, z: i64, w: usize, h: usize) -> Vec<i32> {
        let _ = (self.start_salt, self.start_seed);
        let (p, pw) = bordered_parent(&*self.parent, x, z, w, h);
        let mut out = vec![0i32; w * h];
        for j in 0..h {
            for i in 0..w {
                let v11 = p[(i + 1) + (j + 1) * pw];
                let n = [
                    p[(i + 1) + j * pw],       // N
                    p[(i + 2) + (j + 1) * pw], // E
                    p[i + (j + 1) * pw],       // W
                    p[(i + 1) + (j + 2) * pw], // S
                ];
                let edged = replace_edge(n, v11, WOODED_BADLANDS_PLATEAU, BADLANDS)
                    .or_else(|| replace_edge(n, v11, BADLANDS_PLATEAU, BADLANDS))
                    .or_else(|| replace_edge(n, v11, GIANT_TREE_TAIGA, TAIGA));
                out[i + j * w] = match edged {
                    Some(v) => v,
                    None => {
                        if v11 == DESERT {
                            if n.contains(&SNOWY_TUNDRA) {
                                WOODED_MOUNTAINS
                            } else {
                                v11
                            }
                        } else if v11 == SWAMP {
                            if n.contains(&DESERT)
                                || n.contains(&SNOWY_TAIGA)
                                || n.contains(&SNOWY_TUNDRA)
                            {
                                PLAINS
                            } else if n.contains(&JUNGLE) {
                                JUNGLE_EDGE
                            } else {
                                v11
                            }
                        } else {
                            v11
                        }
                    }
                };
            }
        }
        out
    }
}

// River-init noise: non-ocean cells get a large random value (feeds hills + the
// river branch); ocean cells stay 0. Same-scale (1:1).
salted_layer!(RiverInit);
impl Layer for RiverInit {
    fn gen(&self, x: i64, z: i64, w: usize, h: usize) -> Vec<i32> {
        let _ = self.start_salt;
        let src = self.parent.gen(x, z, w, h);
        let mut out = vec![0i32; w * h];
        for j in 0..h {
            for i in 0..w {
                let idx = i + j * w;
                out[idx] = if src[idx] > 0 {
                    let cs = cell_seed(self.start_seed, x + i as i64, z + j as i64);
                    first_int(cs, 299999) + 2
                } else {
                    0
                };
            }
        }
        out
    }
}

// Rare biome: plains has a 1-in-57 chance to become sunflower plains. 1:1.
salted_layer!(Sunflower);
impl Layer for Sunflower {
    fn gen(&self, x: i64, z: i64, w: usize, h: usize) -> Vec<i32> {
        let _ = self.start_salt;
        let mut out = self.parent.gen(x, z, w, h);
        for j in 0..h {
            for i in 0..w {
                let idx = i + j * w;
                if out[idx] == PLAINS {
                    let cs = cell_seed(self.start_seed, x + i as i64, z + j as i64);
                    if first_is_zero(cs, 57) {
                        out[idx] = SUNFLOWER_PLAINS;
                    }
                }
            }
        }
        out
    }
}

// Smooth: removes single-cell diagonal noise by matching opposite neighbours.
salted_layer!(Smooth);
impl Layer for Smooth {
    fn gen(&self, x: i64, z: i64, w: usize, h: usize) -> Vec<i32> {
        let _ = self.start_salt;
        let (p, pw) = bordered_parent(&*self.parent, x, z, w, h);
        let mut out = vec![0i32; w * h];
        for j in 0..h {
            for i in 0..w {
                let v11 = p[(i + 1) + (j + 1) * pw];
                let west = p[i + (j + 1) * pw];
                let north = p[(i + 1) + j * pw];
                let mut v = v11;
                if v11 != west || v11 != north {
                    let east = p[(i + 2) + (j + 1) * pw];
                    let south = p[(i + 1) + (j + 2) * pw];
                    if west == east && north == south {
                        let cs = cell_seed(self.start_seed, x + i as i64, z + j as i64);
                        v = if cs & (1i64 << 24) != 0 { north } else { west };
                    } else {
                        if west == east {
                            v = west;
                        }
                        if north == south {
                            v = north;
                        }
                    }
                }
                out[i + j * w] = v;
            }
        }
        out
    }
}

#[inline]
fn reduce_id(id: i32) -> i32 {
    if id >= 2 {
        2 + (id & 1)
    } else {
        id
    }
}

// River: where the reduced biome class differs from any orthogonal neighbour, a
// river runs (7); otherwise a no-river sentinel (-1). No RNG (1.7+).
salted_layer!(River);
impl Layer for River {
    fn gen(&self, x: i64, z: i64, w: usize, h: usize) -> Vec<i32> {
        let _ = (self.start_salt, self.start_seed);
        let (p, pw) = bordered_parent(&*self.parent, x, z, w, h);
        let mut out = vec![0i32; w * h];
        for j in 0..h {
            for i in 0..w {
                let c = reduce_id(p[(i + 1) + (j + 1) * pw]);
                let west = reduce_id(p[i + (j + 1) * pw]);
                let east = reduce_id(p[(i + 2) + (j + 1) * pw]);
                let north = reduce_id(p[(i + 1) + j * pw]);
                let south = reduce_id(p[(i + 1) + (j + 2) * pw]);
                out[i + j * w] = if c == west && c == north && c == south && c == east {
                    -1
                } else {
                    RIVER
                };
            }
        }
        out
    }
}

/// A two-parent layer (start salt/seed + biome and river parents).
macro_rules! two_parent_layer {
    ($name:ident) => {
        pub struct $name {
            start_salt: i64,
            start_seed: i64,
            biome: Box<dyn Layer>,
            river: Box<dyn Layer>,
        }
        impl $name {
            pub fn new(
                world_seed: i64,
                salt: i64,
                biome: Box<dyn Layer>,
                river: Box<dyn Layer>,
            ) -> Self {
                let r = LayerRng::new(world_seed, salt);
                Self {
                    start_salt: r.start_salt(),
                    start_seed: r.start_seed(),
                    biome,
                    river,
                }
            }
        }
    };
}

two_parent_layer!(Hills);
impl Layer for Hills {
    fn gen(&self, x: i64, z: i64, w: usize, h: usize) -> Vec<i32> {
        let pw = w + 2;
        let a = self.biome.gen(x - 1, z - 1, pw, h + 2);
        let b = self.river.gen(x - 1, z - 1, pw, h + 2);
        let (ss, st) = (self.start_seed, self.start_salt);
        let mut out = vec![0i32; w * h];
        for j in 0..h {
            for i in 0..w {
                let a11 = a[(i + 1) + (j + 1) * pw];
                let b11 = b[(i + 1) + (j + 1) * pw];
                let bn = (b11 - 2) % 29; // truncating, matches the reference
                let v = if bn == 1 && b11 >= 2 && !is_shallow_ocean(a11) {
                    let m = mutated(a11);
                    if m > 0 {
                        m
                    } else {
                        a11
                    }
                } else {
                    let mut cs = cell_seed(ss, x + i as i64, z + j as i64);
                    if bn == 0 || first_is_zero(cs, 3) {
                        let mut hill = a11;
                        match a11 {
                            DESERT => hill = DESERT_HILLS,
                            FOREST => hill = WOODED_HILLS,
                            BIRCH_FOREST => hill = BIRCH_FOREST_HILLS,
                            DARK_FOREST => hill = PLAINS,
                            TAIGA => hill = TAIGA_HILLS,
                            GIANT_TREE_TAIGA => hill = GIANT_TREE_TAIGA_HILLS,
                            SNOWY_TAIGA => hill = SNOWY_TAIGA_HILLS,
                            PLAINS => {
                                cs = step(cs, st);
                                hill = if first_is_zero(cs, 3) {
                                    WOODED_HILLS
                                } else {
                                    FOREST
                                };
                            }
                            SNOWY_TUNDRA => hill = SNOWY_MOUNTAINS,
                            JUNGLE => hill = JUNGLE_HILLS,
                            OCEAN => hill = DEEP_OCEAN,
                            MOUNTAINS => hill = WOODED_MOUNTAINS,
                            SAVANNA => hill = SAVANNA_PLATEAU,
                            _ => {
                                if are_similar(a11, WOODED_BADLANDS_PLATEAU) {
                                    hill = BADLANDS;
                                } else if a11 == DEEP_OCEAN {
                                    cs = step(cs, st);
                                    if first_is_zero(cs, 3) {
                                        cs = step(cs, st);
                                        hill = if first_is_zero(cs, 2) { PLAINS } else { FOREST };
                                    }
                                }
                            }
                        }
                        if bn == 0 && hill != a11 {
                            hill = mutated(hill);
                            if hill < 0 {
                                hill = a11;
                            }
                        }
                        if hill != a11 {
                            let n = [
                                a[(i + 1) + j * pw],
                                a[(i + 2) + (j + 1) * pw],
                                a[i + (j + 1) * pw],
                                a[(i + 1) + (j + 2) * pw],
                            ];
                            let equals = n.iter().filter(|&&v| are_similar(v, a11)).count();
                            if equals >= 3 {
                                hill
                            } else {
                                a11
                            }
                        } else {
                            a11
                        }
                    } else {
                        a11
                    }
                };
                out[i + j * w] = v;
            }
        }
        out
    }
}

#[inline]
fn is_all4_jfto(n: [i32; 4]) -> bool {
    n.iter().all(|&v| {
        category(v) == JUNGLE || v == FOREST || v == TAIGA || is_oceanic(v)
    })
}

// Shore: beaches, jungle edges, stone/snowy shores, badlands→desert margins.
salted_layer!(Shore);
impl Layer for Shore {
    fn gen(&self, x: i64, z: i64, w: usize, h: usize) -> Vec<i32> {
        let _ = (self.start_salt, self.start_seed);
        let (p, pw) = bordered_parent(&*self.parent, x, z, w, h);
        let mut out = vec![0i32; w * h];
        for j in 0..h {
            for i in 0..w {
                let v11 = p[(i + 1) + (j + 1) * pw];
                let n = [
                    p[(i + 1) + j * pw],
                    p[(i + 2) + (j + 1) * pw],
                    p[i + (j + 1) * pw],
                    p[(i + 1) + (j + 2) * pw],
                ];
                let any_ocean = n.contains(&OCEAN);
                let any_oceanic = n.iter().any(|&v| is_oceanic(v));
                out[i + j * w] = if v11 == MUSHROOM_FIELDS {
                    if any_ocean {
                        MUSHROOM_FIELD_SHORE
                    } else {
                        v11
                    }
                } else if category(v11) == JUNGLE {
                    if is_all4_jfto(n) {
                        if any_oceanic {
                            BEACH
                        } else {
                            v11
                        }
                    } else {
                        JUNGLE_EDGE
                    }
                } else if v11 == MOUNTAINS || v11 == WOODED_MOUNTAINS {
                    if any_oceanic {
                        STONE_SHORE
                    } else {
                        v11
                    }
                } else if is_snowy(v11) {
                    if any_oceanic {
                        SNOWY_BEACH
                    } else {
                        v11
                    }
                } else if v11 == BADLANDS || v11 == WOODED_BADLANDS_PLATEAU {
                    if !any_oceanic {
                        if n.iter().all(|&v| is_mesa(v)) {
                            v11
                        } else {
                            DESERT
                        }
                    } else {
                        v11
                    }
                } else if v11 != OCEAN && v11 != DEEP_OCEAN && v11 != RIVER && v11 != SWAMP {
                    if any_oceanic {
                        BEACH
                    } else {
                        v11
                    }
                } else {
                    v11
                };
            }
        }
        out
    }
}

two_parent_layer!(RiverMix);
impl Layer for RiverMix {
    fn gen(&self, x: i64, z: i64, w: usize, h: usize) -> Vec<i32> {
        let _ = (self.start_salt, self.start_seed);
        let mut out = self.biome.gen(x, z, w, h);
        let riv = self.river.gen(x, z, w, h);
        for idx in 0..w * h {
            let v = out[idx];
            if riv[idx] == RIVER && v != OCEAN && !is_oceanic(v) {
                out[idx] = if v == SNOWY_TUNDRA {
                    FROZEN_RIVER
                } else if v == MUSHROOM_FIELDS || v == MUSHROOM_FIELD_SHORE {
                    MUSHROOM_FIELD_SHORE
                } else {
                    RIVER
                };
            }
        }
        out
    }
}

/// Voronoi: the final per-block jitter from the 1:4 grid to 1:1 resolution. A 2-D
/// jitter (one offset pair per parent corner, from the cell seed) picks the
/// nearest of the four corners for each block.
pub struct Voronoi {
    start_salt: i64,
    start_seed: i64,
    parent: Box<dyn Layer>,
}
impl Voronoi {
    pub fn new(world_seed: i64, salt: i64, parent: Box<dyn Layer>) -> Self {
        let r = LayerRng::new(world_seed, salt);
        Self {
            start_salt: r.start_salt(),
            start_seed: r.start_seed(),
            parent,
        }
    }
}
impl Layer for Voronoi {
    fn gen(&self, x: i64, z: i64, w: usize, h: usize) -> Vec<i32> {
        let xx = x - 2;
        let zz = z - 2;
        let px = xx >> 2;
        let pz = zz >> 2;
        let pw = ((xx + w as i64) >> 2) - px + 2;
        let ph = ((zz + h as i64) >> 2) - pz + 2;
        let src = self.parent.gen(px, pz, pw as usize, ph as usize);
        let pwu = pw as usize;
        let (ss, st) = (self.start_seed, self.start_salt);
        let off = 40 * 1024i64;
        // Jitter offset pair for a corner at block coords (cx, cz).
        let jit = |cx: i64, cz: i64| -> (i64, i64) {
            let cs = cell_seed(ss, cx, cz);
            let j1 = (first_int(cs, 1024) as i64 - 512) * 36;
            let j2 = (first_int(step(cs, st), 1024) as i64 - 512) * 36;
            (j1, j2)
        };
        let mut out = vec![0i32; w * h];
        for pj in 0..(ph as usize - 1) {
            for pi in 0..(pwu - 1) {
                let v00 = src[pi + pj * pwu];
                let v01 = src[pi + (pj + 1) * pwu]; // south
                let v10 = src[(pi + 1) + pj * pwu]; // east
                let v11 = src[(pi + 1) + (pj + 1) * pwu];
                let pix = px + pi as i64;
                let pjz = pz + pj as i64;
                let i4 = pix * 4 - xx;
                let j4 = pjz * 4 - zz;
                let (da1, da2) = jit(pix * 4, pjz * 4);
                let (mut db1, db2) = jit((pix + 1) * 4, pjz * 4);
                db1 += off;
                let (dc1, mut dc2) = jit(pix * 4, (pjz + 1) * 4);
                dc2 += off;
                let (mut dd1, mut dd2) = jit((pix + 1) * 4, (pjz + 1) * 4);
                dd1 += off;
                dd2 += off;
                for jj in 0..4i64 {
                    let oj = j4 + jj;
                    if oj < 0 || oj >= h as i64 {
                        continue;
                    }
                    let mj = 10240 * jj;
                    let (sja, sjb, sjc, sjd) = (
                        (mj - da2) * (mj - da2),
                        (mj - db2) * (mj - db2),
                        (mj - dc2) * (mj - dc2),
                        (mj - dd2) * (mj - dd2),
                    );
                    for ii in 0..4i64 {
                        let oi = i4 + ii;
                        if oi < 0 || oi >= w as i64 {
                            continue;
                        }
                        let mi = 10240 * ii;
                        let da = (mi - da1) * (mi - da1) + sja;
                        let db = (mi - db1) * (mi - db1) + sjb;
                        let dc = (mi - dc1) * (mi - dc1) + sjc;
                        let dd = (mi - dd1) * (mi - dd1) + sjd;
                        let v = if da < db && da < dc && da < dd {
                            v00
                        } else if db < da && db < dc && db < dd {
                            v10
                        } else if dc < da && dc < db && dc < dd {
                            v01
                        } else {
                            v11
                        };
                        out[(oj as usize) * w + oi as usize] = v;
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continent_matches_reference() {
        // Reference values dumped from the per-layer oracle (continent layer).
        assert_eq!(Continent::new(1).gen(0, 0, 4, 2), [1, 0, 1, 1, 0, 0, 0, 0]);
        assert_eq!(Continent::new(42).gen(0, 0, 4, 2), [1, 0, 0, 1, 1, 0, 0, 0]);
        assert_eq!(Continent::new(12345).gen(0, 0, 4, 2), [1, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn continent_origin_force_only_inside_area() {
        let bulk = Continent::new(1).gen(100, 100, 8, 8);
        let single = Continent::new(1).gen(103, 105, 1, 1);
        assert_eq!(single[0], bulk[5 * 8 + 3]);
    }

    // Fuzzy zoom (salt 2000) over the continent layer == reference "zoom2048".
    fn fuzzy_zoom(seed: i64) -> Zoom {
        Zoom::new(seed, 2000, true, Box::new(Continent::new(seed)))
    }

    #[test]
    fn fuzzy_zoom_matches_reference() {
        assert_eq!(
            fuzzy_zoom(1).gen(0, 0, 8, 4),
            [1, 1, 0, 1, 1, 1, 1, 1, 1, 1, 0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
             0, 0, 0]
        );
        assert_eq!(
            fuzzy_zoom(42).gen(0, 0, 8, 4),
            [1, 0, 0, 0, 0, 1, 1, 0, 1, 1, 0, 0, 0, 0, 1, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
             0, 0, 0]
        );
    }

    #[test]
    fn fuzzy_zoom_matches_reference_offset_area() {
        // Offset origin exercises the (x&1, z&1) window extraction.
        assert_eq!(
            fuzzy_zoom(1).gen(-3, -2, 6, 4),
            [0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 1, 1, 0, 0, 0, 1, 1, 1, 0]
        );
    }

    // continent -> fuzzy zoom (2000) -> land (salt 1) == reference "land2048".
    fn land_2048(seed: i64) -> Land {
        Land::new(seed, 1, Box::new(fuzzy_zoom(seed)))
    }

    #[test]
    fn land_matches_reference() {
        assert_eq!(
            land_2048(1).gen(0, 0, 8, 4),
            [1, 1, 1, 0, 1, 1, 1, 1, 1, 1, 0, 0, 1, 1, 0, 0, 0, 0, 0, 1, 1, 0, 1, 1, 0, 0, 0, 0, 0,
             0, 1, 1]
        );
        assert_eq!(
            land_2048(42).gen(0, 0, 8, 4),
            [1, 1, 0, 0, 0, 0, 0, 1, 0, 1, 0, 0, 0, 0, 1, 1, 1, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 1, 0,
             0, 0, 0]
        );
    }

    #[test]
    fn land_matches_reference_offset_area() {
        assert_eq!(
            land_2048(1).gen(-5, -3, 6, 5),
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 0, 0, 1, 1, 1, 0, 0, 1, 0, 0,
             1]
        );
    }

    /// The full cascade up to biome assignment (scale 256), in the 1.8 order/salts.
    fn biome_stack(seed: i64) -> Box<dyn Layer> {
        let l: Box<dyn Layer> = Box::new(Continent::new(seed));
        let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 2000, true, l)); // fuzzy
        let l: Box<dyn Layer> = Box::new(Land::new(seed, 1, l));
        let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 2001, false, l));
        let l: Box<dyn Layer> = Box::new(Land::new(seed, 2, l));
        let l: Box<dyn Layer> = Box::new(Land::new(seed, 50, l));
        let l: Box<dyn Layer> = Box::new(Land::new(seed, 70, l));
        let l: Box<dyn Layer> = Box::new(RemoveOcean::new(seed, 2, l));
        let l: Box<dyn Layer> = Box::new(Snow::new(seed, 2, l));
        let l: Box<dyn Layer> = Box::new(Land::new(seed, 3, l));
        let l: Box<dyn Layer> = Box::new(Cool::new(l));
        let l: Box<dyn Layer> = Box::new(Heat::new(l));
        let l: Box<dyn Layer> = Box::new(Special::new(seed, 3, l));
        let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 2002, false, l));
        let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 2003, false, l));
        let l: Box<dyn Layer> = Box::new(Land::new(seed, 4, l));
        let l: Box<dyn Layer> = Box::new(Mushroom::new(seed, 5, l));
        let l: Box<dyn Layer> = Box::new(DeepOcean::new(l));
        Box::new(Biome::new(seed, 200, l))
    }

    #[test]
    fn biome_assignment_matches_reference() {
        // Full chain (continent..biome) bit-exact vs the per-layer oracle.
        assert_eq!(
            biome_stack(0).gen(0, 0, 4, 4),
            [1, 0, 24, 24, 0, 3, 0, 0, 0, 0, 0, 0, 6, 0, 2, 0]
        );
        assert_eq!(
            biome_stack(1).gen(0, 0, 4, 4),
            [0, 21, 21, 21, 1, 4, 21, 3, 27, 4, 4, 3, 4, 3, 4, 4]
        );
        assert_eq!(
            biome_stack(42).gen(0, 0, 4, 4),
            [12, 30, 12, 12, 12, 12, 27, 4, 30, 0, 3, 3, 0, 0, 3, 29]
        );
    }

    /// continent..biome -> zoom128(1000) -> zoom64(1001) -> biomeEdge(1000).
    fn biome_edge_stack(seed: i64) -> Box<dyn Layer> {
        let l = biome_stack(seed);
        let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 1000, false, l));
        let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 1001, false, l));
        Box::new(BiomeEdge::new(seed, 1000, l))
    }

    #[test]
    fn biome_edge_matches_reference() {
        assert_eq!(
            biome_edge_stack(0).gen(0, 0, 8, 4),
            [1, 1, 1, 0, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 1, 0, 3, 0, 0,
             0, 0, 0]
        );
        assert_eq!(
            biome_edge_stack(1).gen(0, 0, 8, 4),
            [0, 0, 0, 21, 21, 21, 21, 21, 0, 0, 4, 21, 21, 21, 21, 21, 0, 0, 4, 21, 21, 21, 21, 21,
             1, 4, 4, 4, 4, 21, 21, 21]
        );
        assert_eq!(
            biome_edge_stack(42).gen(0, 0, 8, 4),
            [12, 12, 12, 30, 30, 30, 30, 12, 12, 12, 12, 12, 30, 30, 30, 12, 12, 12, 12, 12, 30, 30,
             12, 12, 12, 12, 12, 12, 30, 12, 12, 12]
        );
    }

    // --- Full stack: the complete 1.8 biome cascade (incl. rivers + voronoi). ---

    fn deep_ocean_256(seed: i64) -> Box<dyn Layer> {
        let l: Box<dyn Layer> = Box::new(Continent::new(seed));
        let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 2000, true, l));
        let l: Box<dyn Layer> = Box::new(Land::new(seed, 1, l));
        let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 2001, false, l));
        let l: Box<dyn Layer> = Box::new(Land::new(seed, 2, l));
        let l: Box<dyn Layer> = Box::new(Land::new(seed, 50, l));
        let l: Box<dyn Layer> = Box::new(Land::new(seed, 70, l));
        let l: Box<dyn Layer> = Box::new(RemoveOcean::new(seed, 2, l));
        let l: Box<dyn Layer> = Box::new(Snow::new(seed, 2, l));
        let l: Box<dyn Layer> = Box::new(Land::new(seed, 3, l));
        let l: Box<dyn Layer> = Box::new(Cool::new(l));
        let l: Box<dyn Layer> = Box::new(Heat::new(l));
        let l: Box<dyn Layer> = Box::new(Special::new(seed, 3, l));
        let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 2002, false, l));
        let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 2003, false, l));
        let l: Box<dyn Layer> = Box::new(Land::new(seed, 4, l));
        let l: Box<dyn Layer> = Box::new(Mushroom::new(seed, 5, l));
        Box::new(DeepOcean::new(l))
    }

    fn river_init(seed: i64) -> Box<dyn Layer> {
        Box::new(RiverInit::new(seed, 100, deep_ocean_256(seed)))
    }

    fn hills_branch(seed: i64) -> Box<dyn Layer> {
        // The hills-branch zooms use salt 0 (zero-init) for this ruleset.
        let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 0, false, river_init(seed)));
        Box::new(Zoom::new(seed, 0, false, l))
    }

    fn main_branch(seed: i64) -> Box<dyn Layer> {
        let l: Box<dyn Layer> =
            Box::new(Hills::new(seed, 1000, biome_edge_stack(seed), hills_branch(seed)));
        let l: Box<dyn Layer> = Box::new(Sunflower::new(seed, 1001, l));
        let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 1000, false, l));
        let l: Box<dyn Layer> = Box::new(Land::new(seed, 3, l));
        let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 1001, false, l));
        let l: Box<dyn Layer> = Box::new(Shore::new(seed, 1000, l));
        let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 1002, false, l));
        let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 1003, false, l));
        Box::new(Smooth::new(seed, 1000, l))
    }

    fn river_branch(seed: i64) -> Box<dyn Layer> {
        let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 1000, false, river_init(seed)));
        let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 1001, false, l));
        let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 1000, false, l));
        let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 1001, false, l));
        let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 1002, false, l));
        let l: Box<dyn Layer> = Box::new(Zoom::new(seed, 1003, false, l));
        let l: Box<dyn Layer> = Box::new(River::new(seed, 1, l));
        Box::new(Smooth::new(seed, 1000, l))
    }

    fn river_mix(seed: i64) -> Box<dyn Layer> {
        Box::new(RiverMix::new(seed, 100, main_branch(seed), river_branch(seed)))
    }

    fn full_biomes(seed: i64) -> Box<dyn Layer> {
        Box::new(Voronoi::new(seed, 10, river_mix(seed)))
    }

    fn fnv(ids: &[i32]) -> u64 {
        let mut h: u64 = 1469598103934665603;
        for &id in ids {
            h = h.wrapping_mul(1099511628211).wrapping_add(id as i64 as u64);
        }
        h
    }

    #[test]
    fn full_biomes_match_reference() {
        // Final per-block biome (river-mix + voronoi) over (0,0,128,128), hashed
        // and compared to the reference's per-block biome over the same region.
        // This region contains plains/forest/beach/ocean and ~415 river cells.
        for &(seed, want) in &[
            (0i64, 18095362520938780919u64),
            (1, 16764737903282282348),
            (42, 859484148017061748),
            (7, 10509617721058722691),
        ] {
            assert_eq!(
                fnv(&full_biomes(seed).gen(0, 0, 128, 128)),
                want,
                "final biome mismatch for seed {seed}"
            );
        }
    }

    #[test]
    fn full_biomes_match_reference_far_negative_offset() {
        // A region far from origin with negative coordinates — catches offset and
        // sign bugs that an at-origin test would miss.
        assert_eq!(
            fnv(&full_biomes(12345).gen(-500, 300, 64, 64)),
            16344384467930091955,
            "final biome mismatch at far negative offset"
        );
    }
}
