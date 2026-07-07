//! Column-gen cache: the "Optimize explored terrain" world setting persists
//! each explored column's 2D worldgen result (the slimmed `ColumnGen` — biome,
//! surfaces, range scalars) so a revisit skips the heavy per-column noise job,
//! which measures ~70% of worldgen cost.
//!
//! This is a disposable CACHE of deterministic data, not authoritative world
//! state: records are seed- and version-stamped, and any mismatch or corruption
//! falls through to normal generation. It therefore lives in its own `colgen/`
//! directory beside `region/` (same container format, `g.<rx>.<rz>.dat`),
//! keeping the authoritative region files pure.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use crate::chunk::{ChunkPos, SECTION_SIZE};
use crate::save::codec::{deflate, inflate, put_u32, put_u8, Reader};
use crate::save::region::{REGION_SHIFT, REGION_SIZE};

const VERSION: u8 = 1;
const CELLS: usize = SECTION_SIZE * SECTION_SIZE;

/// One column's cached 2D gen data — exactly the resident (slimmed) `ColumnGen`
/// payload. Built by `ColumnGen::cache_record` and consumed by
/// `ColumnGen::from_cache_record` (worldgen::driver owns the field semantics).
pub struct ColumnGenRecord {
    pub pos: ChunkPos,
    pub seed: u32,
    pub biome: Box<[u8]>,
    pub surf: Box<[i32]>,
    pub top_surf: Box<[i32]>,
    pub surf_min: i32,
    pub surf_max: i32,
    pub cand_surf_min: i32,
    pub cand_surf_max: i32,
    pub content_top: i32,
}

pub fn region_of(pos: ChunkPos) -> (i32, i32) {
    (pos.cx >> REGION_SHIFT, pos.cz >> REGION_SHIFT)
}

pub fn local_index(pos: ChunkPos) -> u16 {
    let lx = (pos.cx & (REGION_SIZE - 1)) as u16;
    let lz = (pos.cz & (REGION_SIZE - 1)) as u16;
    (lz << REGION_SHIFT) | lx
}

pub fn column_pos(rx: i32, rz: i32, lidx: u16) -> ChunkPos {
    let mask = (REGION_SIZE - 1) as u16;
    let lx = (lidx & mask) as i32;
    let lz = ((lidx >> REGION_SHIFT) & mask) as i32;
    ChunkPos::new(rx * REGION_SIZE + lx, rz * REGION_SIZE + lz)
}

pub fn cache_path(colgen_dir: &Path, rx: i32, rz: i32) -> PathBuf {
    colgen_dir.join(format!("g.{rx}.{rz}.dat"))
}

/// Parse `g.<rx>.<rz>.dat` back into region coords (handles negatives).
pub fn parse_cache_name(path: &Path) -> Option<(i32, i32)> {
    let name = path.file_name()?.to_str()?;
    let rest = name.strip_prefix("g.")?.strip_suffix(".dat")?;
    let (a, b) = rest.split_once('.')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// Encode one record: `[version, seed, biome, surf, top_surf, scalars]`, deflated.
pub fn encode_record(rec: &ColumnGenRecord) -> Vec<u8> {
    debug_assert_eq!(rec.biome.len(), CELLS);
    debug_assert_eq!(rec.surf.len(), CELLS);
    debug_assert_eq!(rec.top_surf.len(), CELLS);
    let mut payload = Vec::with_capacity(1 + 4 + CELLS * 9 + 20);
    put_u8(&mut payload, VERSION);
    put_u32(&mut payload, rec.seed);
    payload.extend_from_slice(&rec.biome);
    for &v in rec.surf.iter().chain(rec.top_surf.iter()) {
        put_u32(&mut payload, v as u32);
    }
    for v in [
        rec.surf_min,
        rec.surf_max,
        rec.cand_surf_min,
        rec.cand_surf_max,
        rec.content_top,
    ] {
        put_u32(&mut payload, v as u32);
    }
    deflate(&payload)
}

/// Decode a record for `pos`. `None` (regenerate instead) on any corruption,
/// version drift, or a seed that doesn't match the live world.
pub fn decode_record(pos: ChunkPos, seed: u32, blob: &[u8]) -> Option<ColumnGenRecord> {
    let payload = inflate(blob)?;
    let mut r = Reader::new(&payload);
    if r.u8()? != VERSION || r.u32()? != seed {
        return None;
    }
    let biome: Box<[u8]> = r.bytes(CELLS)?.into();
    let mut i32s = |n: usize| -> Option<Box<[i32]>> {
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            out.push(r.u32()? as i32);
        }
        Some(out.into_boxed_slice())
    };
    let surf = i32s(CELLS)?;
    let top_surf = i32s(CELLS)?;
    let mut scalar = || -> Option<i32> { Some(r.u32()? as i32) };
    Some(ColumnGenRecord {
        pos,
        seed,
        biome,
        surf,
        top_surf,
        surf_min: scalar()?,
        surf_max: scalar()?,
        cand_surf_min: scalar()?,
        cand_surf_max: scalar()?,
        content_top: scalar()?,
    })
}

/// The present column positions in one cache file (for the open-time manifest).
pub fn read_cache_indices(path: &Path) -> io::Result<Vec<u16>> {
    Ok(crate::save::region::read_region(path)?
        .into_keys()
        .collect())
}

/// Merge records into their cache files (read-modify-write per file), mirroring
/// `write_sections`. Reuses the region container format verbatim. Returns the
/// paths written, so the I/O thread's read cache can refresh.
pub fn write_records(colgen_dir: &Path, recs: Vec<ColumnGenRecord>) -> Vec<PathBuf> {
    let mut by_region: HashMap<(i32, i32), Vec<ColumnGenRecord>> = HashMap::new();
    for rec in recs {
        by_region.entry(region_of(rec.pos)).or_default().push(rec);
    }
    let mut touched = Vec::with_capacity(by_region.len());
    for ((rx, rz), group) in by_region {
        let path = cache_path(colgen_dir, rx, rz);
        // A corrupt cache file starts fresh — it is derived data.
        let mut records = crate::save::region::read_region(&path).unwrap_or_default();
        for rec in &group {
            records.insert(local_index(rec.pos), encode_record(rec));
        }
        let _ = crate::save::region::write_region(&path, &records);
        touched.push(path);
    }
    touched
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn column_index_roundtrips() {
        for &(cx, cz) in &[(0, 0), (-1, -1), (31, 31), (-32, 32), (100, -77)] {
            let pos = ChunkPos::new(cx, cz);
            let (rx, rz) = region_of(pos);
            assert_eq!(column_pos(rx, rz, local_index(pos)), pos, "({cx},{cz})");
        }
    }

    #[test]
    fn record_roundtrips_and_rejects_seed_or_version_drift() {
        let rec = ColumnGenRecord {
            pos: ChunkPos::new(3, -7),
            seed: 0xDEAD_BEEF,
            biome: vec![7u8; CELLS].into(),
            surf: (0..CELLS as i32).map(|i| i - 64).collect(),
            top_surf: (0..CELLS as i32).map(|i| i - 70).collect(),
            surf_min: -64,
            surf_max: 191,
            cand_surf_min: -66,
            cand_surf_max: 200,
            content_top: 231,
        };
        let blob = encode_record(&rec);
        let back = decode_record(rec.pos, rec.seed, &blob).expect("roundtrip");
        assert_eq!(back.biome, rec.biome);
        assert_eq!(back.surf, rec.surf);
        assert_eq!(back.top_surf, rec.top_surf);
        assert_eq!(
            (
                back.surf_min,
                back.surf_max,
                back.cand_surf_min,
                back.cand_surf_max,
                back.content_top
            ),
            (
                rec.surf_min,
                rec.surf_max,
                rec.cand_surf_min,
                rec.cand_surf_max,
                rec.content_top
            )
        );
        assert!(
            decode_record(rec.pos, rec.seed ^ 1, &blob).is_none(),
            "a different world seed must reject the cached record"
        );
        assert!(decode_record(rec.pos, rec.seed, b"junk").is_none());
    }
}
