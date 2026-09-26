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
use std::sync::Arc;

use petramond_region::{REGION_SHIFT, REGION_SIZE};
use petramond_util::bytecodec::{deflate, inflate, put_u32, put_u64, put_u8, Reader};
use petramond_world::chunk::{ChunkPos, SECTION_SIZE};

/// Layout of an encoded record. Bump it when [`ColumnCore`]'s fields or
/// their encoding change; what the ENGINE generates is stamped separately
/// (see [`ENGINE_STAMP`]), so a generation change needs no manual bump here.
pub const FORMAT_VERSION: u8 = 12;

/// What the engine generates from the shipped catalogs: the worldgen parity
/// hash, which the `genparity` CI gate forces every generation-changing commit
/// to update. Stamping it means a record written by an engine that produced
/// different columns is never served, without anyone remembering to bump
/// [`FORMAT_VERSION`].
pub const ENGINE_STAMP: u64 = crate::parity::EXPECTED_COMBINED;

/// Fingerprint of the loaded catalogs — habitat, excavation, climate
/// placement and terrain recipe — stamped beside the seed: the same content
/// identity every in-memory memo keys on
/// ([`GenContext`](crate::cache::GenContext)). [`ENGINE_STAMP`] only catches
/// ENGINE drift; installing, removing, or retuning a pack that reshapes caves
/// or terrain or moves a biome changes no engine code but does change `surf` /
/// `top_surf` / `biome`, and without this the cache would happily serve the
/// stale columns.
fn table_fingerprint(seed: u32) -> u64 {
    crate::cache::GenContext::installed(seed).tables()
}
const CELLS: usize = SECTION_SIZE * SECTION_SIZE;
/// How many cells the tint halo reaches beyond each X/Z edge of the column.
pub const MESH_BIOME_RADIUS: usize = 2;
/// Side of the tint halo.
pub const MESH_BIOME_SIDE: usize = SECTION_SIZE + 2 * MESH_BIOME_RADIUS;
const MESH_BIOME_CELLS: usize = MESH_BIOME_SIDE * MESH_BIOME_SIDE;

/// One column's resident 2D gen data: what a `ColumnGen` keeps for the
/// column's lifetime and, field for field, what a cache record persists. The
/// generator owns the field semantics (`worldgen::driver`); this type owns
/// their encoding, so a new column field is added here once, and the encoder
/// and decoder below — which name every field — refuse to compile until it
/// is encoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnCore {
    /// Biome id per `(x,z)` in the column's 16×16, indexed `z*16 + x`.
    pub biome: Box<[u8]>,
    /// The [`MESH_BIOME_SIDE`]² tint halo, captured from the column-generation
    /// region so mesh submission never runs analytical biome generation on
    /// the owning thread.
    pub mesh_biome: Arc<[u8]>,
    /// Density top-solid surface (world Y, or `-1` for a floorless column)
    /// per `(x,z)`, indexed `z*16 + x`.
    pub surf: Box<[i32]>,
    /// Post-cave bare top non-air surface per `(x,z)`, before
    /// vegetation/trees. Lower than `surf` only at cave entrances.
    pub top_surf: Box<[i32]>,
    pub surf_min: i32,
    pub surf_max: i32,
    /// Surface min/max across the whole feature candidate window (chunk +
    /// spacing margin), so tree gating accounts for anchors at margin origins
    /// and content reaching in from neighbours, not just this 16×16.
    pub cand_surf_min: i32,
    pub cand_surf_max: i32,
    /// Highest world Y that can hold any generated block in this column.
    /// Sections whose floor is above it are provably all-air sky.
    pub content_top: i32,
}

impl ColumnCore {
    /// Encoded size, for the payload buffer.
    const ENCODED_LEN: usize = CELLS + MESH_BIOME_CELLS + CELLS * 8 + 5 * 4;

    /// Append every field, in declaration order.
    fn encode(&self, out: &mut Vec<u8>) {
        let ColumnCore {
            biome,
            mesh_biome,
            surf,
            top_surf,
            surf_min,
            surf_max,
            cand_surf_min,
            cand_surf_max,
            content_top,
        } = self;
        debug_assert_eq!(biome.len(), CELLS);
        debug_assert_eq!(mesh_biome.len(), MESH_BIOME_CELLS);
        debug_assert_eq!(surf.len(), CELLS);
        debug_assert_eq!(top_surf.len(), CELLS);
        out.extend_from_slice(biome);
        out.extend_from_slice(mesh_biome);
        for &v in surf.iter().chain(top_surf.iter()) {
            put_u32(out, v as u32);
        }
        for &v in [surf_min, surf_max, cand_surf_min, cand_surf_max, content_top] {
            put_u32(out, v as u32);
        }
    }

    /// Read back what [`encode`](Self::encode) wrote; `None` on a short or
    /// malformed payload.
    fn decode(r: &mut Reader) -> Option<Self> {
        let biome: Box<[u8]> = r.bytes(CELLS)?.into();
        let mesh_biome: Arc<[u8]> = Arc::from(r.bytes(MESH_BIOME_CELLS)?);
        let mut i32s =
            |n: usize| -> Option<Box<[i32]>> { (0..n).map(|_| r.u32().map(|v| v as i32)).collect() };
        let surf = i32s(CELLS)?;
        let top_surf = i32s(CELLS)?;
        let mut scalar = || -> Option<i32> { Some(r.u32()? as i32) };
        Some(ColumnCore {
            biome,
            mesh_biome,
            surf,
            top_surf,
            surf_min: scalar()?,
            surf_max: scalar()?,
            cand_surf_min: scalar()?,
            cand_surf_max: scalar()?,
            content_top: scalar()?,
        })
    }
}

/// One column's cache record: its position, the seed that generated it, and
/// its [`ColumnCore`]. Built by `ColumnGen::cache_record` and consumed by
/// `ColumnGen::from_cache_record`.
pub struct ColumnGenRecord {
    pub pos: ChunkPos,
    pub seed: u32,
    pub core: ColumnCore,
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

/// Encode one record: `[format version, engine stamp, seed, table
/// fingerprint, core]`, deflated.
pub fn encode_record(rec: &ColumnGenRecord) -> Vec<u8> {
    let mut payload = Vec::with_capacity(1 + 8 + 4 + 8 + ColumnCore::ENCODED_LEN);
    put_u8(&mut payload, FORMAT_VERSION);
    put_u64(&mut payload, ENGINE_STAMP);
    put_u32(&mut payload, rec.seed);
    put_u64(&mut payload, table_fingerprint(rec.seed));
    rec.core.encode(&mut payload);
    deflate(&payload)
}

/// Decode a record for `pos`. `None` (regenerate instead) on any corruption,
/// format or engine drift, a seed that doesn't match the live world, or
/// catalogs that no longer match the ones that wrote it.
pub fn decode_record(pos: ChunkPos, seed: u32, blob: &[u8]) -> Option<ColumnGenRecord> {
    let payload = inflate(blob)?;
    let mut r = Reader::new(&payload);
    if r.u8()? != FORMAT_VERSION
        || r.u64()? != ENGINE_STAMP
        || r.u32()? != seed
        || r.u64()? != table_fingerprint(seed)
    {
        return None;
    }
    let core = ColumnCore::decode(&mut r)?;
    Some(ColumnGenRecord { pos, seed, core })
}

/// The present column positions in one cache file (for the open-time manifest).
pub fn read_cache_indices(path: &Path) -> io::Result<Vec<u16>> {
    petramond_region::read_region_indices(path)
}

/// Merge records into their cache files (read-modify-write per file), mirroring
/// `write_sections`. Reuses the region container format verbatim. Returns the
/// paths actually written, so the I/O thread's read cache refreshes only from
/// files that hold the new records. A file that fails to write is logged and
/// left out: the cache is disposable, so its columns simply regenerate.
pub fn write_records(colgen_dir: &Path, recs: Vec<ColumnGenRecord>) -> Vec<PathBuf> {
    let mut by_region: HashMap<(i32, i32), Vec<ColumnGenRecord>> = HashMap::new();
    for rec in recs {
        by_region.entry(region_of(rec.pos)).or_default().push(rec);
    }
    let mut written = Vec::with_capacity(by_region.len());
    for ((rx, rz), group) in by_region {
        let path = cache_path(colgen_dir, rx, rz);
        match write_region(&path, &group) {
            Ok(()) => written.push(path),
            Err(err) => log::warn!(
                "column-gen cache: {} record(s) not written to {}: {err}",
                group.len(),
                path.display()
            ),
        }
    }
    written
}

/// Merge one region's records into its cache file.
fn write_region(path: &Path, group: &[ColumnGenRecord]) -> io::Result<()> {
    let records = group
        .iter()
        .map(|rec| (local_index(rec.pos), encode_record(rec)));
    petramond_region::merge_region(path, records, petramond_region::MergePolicy::Rebuildable)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_record() -> ColumnGenRecord {
        ColumnGenRecord {
            pos: ChunkPos::new(3, -7),
            seed: 0xDEAD_BEEF,
            core: ColumnCore {
                biome: vec![7u8; CELLS].into(),
                mesh_biome: Arc::from(vec![7u8; MESH_BIOME_CELLS].into_boxed_slice()),
                surf: (0..CELLS as i32).map(|i| i - 64).collect(),
                top_surf: (0..CELLS as i32).map(|i| i - 70).collect(),
                surf_min: -64,
                surf_max: 191,
                cand_surf_min: -66,
                cand_surf_max: 200,
                content_top: 231,
            },
        }
    }

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
        let rec = sample_record();
        let blob = encode_record(&rec);
        let back = decode_record(rec.pos, rec.seed, &blob).expect("roundtrip");
        assert_eq!(back.core, rec.core);
        assert!(
            decode_record(rec.pos, rec.seed ^ 1, &blob).is_none(),
            "a different world seed must reject the cached record"
        );
        assert!(decode_record(rec.pos, rec.seed, b"junk").is_none());

        // An old-format record must be rejected outright (regenerate; this is
        // a disposable cache with no upgrade path).
        let mut payload = inflate(&blob).unwrap();
        payload[0] = FORMAT_VERSION - 1;
        assert!(
            decode_record(rec.pos, rec.seed, &deflate(&payload)).is_none(),
            "a stale-format record must be rejected"
        );
    }

    /// A record written by an engine that generated different columns — a
    /// different parity hash — is stale even though its layout still parses.
    #[test]
    fn a_record_from_another_engine_is_rejected() {
        let rec = sample_record();
        let mut payload = inflate(&encode_record(&rec)).unwrap();
        let stamp = 1..9;
        assert_eq!(payload[stamp.clone()], ENGINE_STAMP.to_le_bytes());
        payload[stamp].copy_from_slice(&(ENGINE_STAMP ^ 1).to_le_bytes());
        assert!(decode_record(rec.pos, rec.seed, &deflate(&payload)).is_none());
    }

    #[test]
    fn a_truncated_core_is_rejected() {
        let rec = sample_record();
        let mut payload = inflate(&encode_record(&rec)).unwrap();
        payload.truncate(payload.len() - 1);
        assert!(decode_record(rec.pos, rec.seed, &deflate(&payload)).is_none());
    }

    /// A region that cannot be written is reported by leaving its path out,
    /// so no read cache refreshes from a file that never received the records.
    #[test]
    fn a_failed_region_write_is_not_reported_as_written() {
        let dir = std::env::temp_dir().join(format!(
            "petramond-colgen-write-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ok = sample_record();
        let mut blocked = sample_record();
        blocked.pos = ChunkPos::new(ok.pos.cx + REGION_SIZE, ok.pos.cz);
        // A directory where the blocked region's file should go: the merge
        // cannot open it as a file.
        let (brx, brz) = region_of(blocked.pos);
        std::fs::create_dir_all(cache_path(&dir, brx, brz)).unwrap();

        let written = write_records(&dir, vec![ok, blocked]);
        let (rx, rz) = region_of(sample_record().pos);
        assert_eq!(written, vec![cache_path(&dir, rx, rz)]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
