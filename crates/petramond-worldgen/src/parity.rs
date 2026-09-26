//! The worldgen byte-parity gate: one hash over what the production section
//! pipeline generates for a fixed spread of columns across several seeds.
//!
//! The hash covers exactly what the streamer hands a world: each column's 2D
//! data (biomes, density surfaces, heightmap, content top) and every section
//! from the bottom of the world up to the column's content top, generated
//! through [`ChunkGenerator::generate_section`] — the same
//! [`start_section`](ChunkGenerator::start_section) path the streaming worker
//! runs, including caves below y = 0, fluid falls and the section terrain
//! memo. Sections above the content top are provably air and never
//! generated, so they are not hashed.
//!
//! `make genparity` (a CI gate) compares the hash with [`EXPECTED_COMBINED`].
//! A pure refactor, toolchain bump or rustflag change must reproduce it. A
//! change that means to alter generation updates the constant in the same
//! commit, which makes every such change a deliberate, reviewable one.

use std::sync::Arc;

use petramond_world::chunk::{
    SectionPos, CHUNK_SX, CHUNK_SZ, SECTION_MAX_CY, SECTION_MIN_CY, SECTION_SIZE,
};
use petramond_world::section::Section;

use crate::cache::{CacheBudget, GenCaches};
use crate::driver::ChunkGenerator;

/// The checked-in parity hash of [`combined_hash`] over the shipped catalogs
/// with no packs installed. Update it only together with a change that is
/// meant to alter generation.
pub const EXPECTED_COMBINED: u64 = 0x731e_2503_a634_c9fb;

/// The sampled seeds: chosen so forests, wooded hills, riverbank plains and
/// redwood stands all fall inside the sample.
pub const SEEDS: [u32; 3] = [0x1234_5678, 786, 0xDEAD_BEEF];

/// The sampled columns: every third point of a 25×25 lattice of chunks five
/// apart, a wide spread around the origin.
pub fn sample_columns() -> impl Iterator<Item = (i32, i32)> {
    (-12..=12)
        .flat_map(|cz| (-12..=12).map(move |cx| (cx, cz)))
        .filter(|(cx, cz)| (cx + cz) % 3 == 0)
        .map(|(cx, cz)| (cx * 5, cz * 5))
}

/// FNV-1a over the little-endian bytes of what it is fed.
struct Fnv(u64);

impl Fnv {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    fn new() -> Self {
        Self(Self::OFFSET)
    }

    fn bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(Self::PRIME);
        }
    }

    fn u16(&mut self, v: u16) {
        self.bytes(&v.to_le_bytes());
    }

    fn u32(&mut self, v: u32) {
        self.bytes(&v.to_le_bytes());
    }

    fn i32(&mut self, v: i32) {
        self.bytes(&v.to_le_bytes());
    }
}

/// The pure engine generator for `seed`: no mod hooks and memos of its own,
/// so the hash cannot depend on what else the process installed or cached.
pub fn engine_generator(seed: u32) -> ChunkGenerator {
    ChunkGenerator::with_caches(seed, None, Arc::new(GenCaches::new(CacheBudget::REFERENCE)))
}

/// The parity hash of one column: its 2D data, then each generated section
/// from the bottom of the world up.
pub fn column_hash(generator: &ChunkGenerator, cx: i32, cz: i32) -> u64 {
    let col = generator.generate_column_gen(cx, cz);
    let mut h = Fnv::new();
    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            h.bytes(&[col.biome_at(x, z)]);
            h.i32(col.surface_y(x, z));
            h.i32(col.heightmap_surface_y(x, z));
        }
    }
    h.i32(col.content_top());
    let top = SECTION_MAX_CY.min(col.content_top().div_euclid(SECTION_SIZE as i32));
    for cy in SECTION_MIN_CY..=top {
        let section = generator.generate_section(SectionPos::new(cx, cy, cz), &col);
        hash_section(&mut h, &section);
    }
    h.0
}

/// Everything generation leaves in a section: block ids, fluid levels, cell
/// states and cell data, the sparse maps in cell order.
fn hash_section(h: &mut Fnv, section: &Section) {
    for id in section.blocks_iter() {
        h.u16(id);
    }
    match section.fluid_slice() {
        Some(meta) => {
            h.bytes(&[1]);
            h.bytes(meta);
        }
        None => h.bytes(&[0]),
    }
    let mut states: Vec<_> = section.cell_states().iter().collect();
    states.sort_unstable_by_key(|&(&cell, _)| cell);
    h.u32(states.len() as u32);
    for (&cell, state) in states {
        h.u16(cell);
        h.bytes(&[state.id_mask(), state.bytes().len() as u8]);
        h.bytes(state.bytes());
    }
    let mut data: Vec<_> = section.cell_kv().iter().collect();
    data.sort_unstable_by_key(|&(&cell, _)| cell);
    h.u32(data.len() as u32);
    for (&cell, entries) in data {
        h.u16(cell);
        h.u32(entries.len() as u32);
        for (key, value) in entries {
            h.u32(key.len() as u32);
            h.bytes(key.as_bytes());
            h.u32(value.len() as u32);
            h.bytes(value);
        }
    }
}

/// The parity hash of one seed's sample.
pub fn seed_hash(seed: u32) -> u64 {
    let generator = engine_generator(seed);
    let mut h = Fnv::new();
    for (cx, cz) in sample_columns() {
        h.bytes(&column_hash(&generator, cx, cz).to_le_bytes());
    }
    h.0
}

/// The COMBINED parity hash: every seed's hash, in [`SEEDS`] order. The seeds
/// generate on threads of their own; each is a pure function of its seed, so
/// the result does not depend on scheduling.
pub fn combined_hash() -> u64 {
    let per_seed: Vec<u64> = std::thread::scope(|scope| {
        let workers: Vec<_> = SEEDS
            .iter()
            .map(|&seed| scope.spawn(move || seed_hash(seed)))
            .collect();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a parity worker panicked"))
            .collect()
    });
    let mut combined = Fnv::new();
    for hash in per_seed {
        combined.bytes(&hash.to_le_bytes());
    }
    combined.0
}

#[cfg(all(test, feature = "worldgen-tests"))]
mod tests {
    use super::*;

    /// A column's parity hash is a function of the column alone: warm memos,
    /// a fresh generator and the order columns are asked in change nothing.
    #[test]
    fn a_column_hash_does_not_depend_on_what_is_cached() {
        let seed = SEEDS[1];
        let warmed = engine_generator(seed);
        let first = column_hash(&warmed, 5, -10);
        let _ = column_hash(&warmed, 0, 0);
        assert_eq!(column_hash(&warmed, 5, -10), first);
        assert_eq!(column_hash(&engine_generator(seed), 5, -10), first);
        assert_ne!(column_hash(&engine_generator(SEEDS[0]), 5, -10), first);
    }

    /// The sample is the fixed spread the golden hash was taken over.
    #[test]
    fn the_sample_is_the_pinned_spread() {
        let columns: Vec<_> = sample_columns().collect();
        assert_eq!(columns.len(), 209);
        assert!(columns.contains(&(0, 0)));
        assert!(columns.contains(&(-60, 60)));
        assert!(columns.iter().all(|&(cx, cz)| (cx / 5 + cz / 5) % 3 == 0));
    }
}
