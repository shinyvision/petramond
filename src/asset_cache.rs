//! On-disk cache of *compiled* assets: bake an authored source file (e.g. a Blockbench
//! `.bbmodel`) into a compact, fast-loading binary once, then reuse that binary until the
//! source — or the compiler — changes.
//!
//! # Why
//!
//! Authored formats are made for editing, not loading: a `.bbmodel` is JSON with a
//! base64-PNG texture, so every load means a `serde_json` tree-walk, a base64 decode and
//! an image decode. This module runs that expensive [`compile`](CompiledAsset::compile)
//! step at most once per (source, format) and writes the result as a single self-contained
//! binary keyed by a hash of the source. Loads after that are a `read` + a `bincode`
//! decode — no parsing, no image decode, one file holding geometry + bones + animations +
//! texture. As a bonus the runtime stops caring about the authoring format at all, which
//! is what makes runtime-loaded / modded assets tractable later.
//!
//! # Staleness — the whole game
//!
//! A cache entry is reused only when BOTH agree with the request:
//! - **source hash** — an [FNV-1a](hash_source) digest of the authored bytes; any edit to
//!   the source changes it and forces a rebuild.
//! - **format version** — [`CompiledAsset::FORMAT_VERSION`]; bump it whenever the on-disk
//!   layout or the `compile` logic changes and every stale entry rebuilds itself.
//!
//! Because the entry is *fully regenerable from the source*, that version bump is the ONLY
//! migration mechanism there will ever be: the compiled format is disposable, so it never
//! needs backward-compatibility or migration code. Keep all the care in the source parser
//! and the runtime structs; treat the cache as throwaway.
//!
//! # Robustness
//!
//! Writes are atomic (write a temp sibling, then rename) so a crash or a second process
//! can never leave a half-written file that passes the header check. Any unreadable,
//! truncated, wrong-magic, wrong-version or wrong-hash file is simply treated as a miss and
//! rebuilt. And caching is *best-effort*: if the cache can't be written (read-only or full
//! disk) the freshly compiled asset is still returned — you lose the speed-up, never the
//! asset. The only thing [`load_or_compile`] reports as an error is a genuine *compile*
//! failure (a malformed source).
//!
//! # Extending to a new asset kind
//!
//! Implement [`CompiledAsset`] for the runtime type — a unique [`MAGIC`](CompiledAsset::MAGIC)
//! and [`EXTENSION`](CompiledAsset::EXTENSION) plus its own `compile` — and call
//! [`load_or_compile`]. The container, hashing, versioning and atomic-write machinery are
//! shared, so a future data-driven *block* model baked from the same `.bbmodel` frontend
//! reuses all of this and only writes its own `compile`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::de::DeserializeOwned;
use serde::Serialize;

/// A runtime asset that can be compiled from authored source bytes and cached on disk in a
/// fast binary form. Implementors get [`load_or_compile`] for free.
///
/// The four consts describe the on-disk file: a `MAGIC` tag + `EXTENSION` identify the
/// kind, `SUBDIR` groups the files, and `FORMAT_VERSION` gates compatibility.
pub trait CompiledAsset: Serialize + DeserializeOwned + Sized {
    /// 8-byte tag at the start of every file of this kind. Rejects a file of the wrong
    /// kind (or a non-cache file) outright. Convention: an ASCII name, null-padded.
    const MAGIC: [u8; 8];

    /// Bump whenever the on-disk layout OR [`compile`](Self::compile)'s output changes, so
    /// every stale cache entry is transparently rebuilt. This is the only migration
    /// mechanism — see the module docs.
    const FORMAT_VERSION: u32;

    /// Sub-directory of the cache root these files live in (e.g. `"models"`).
    const SUBDIR: &'static str;

    /// File extension for this kind, without the dot (e.g. `"llmob"`).
    const EXTENSION: &'static str;

    /// Compile authored `source` bytes into the runtime asset: the expensive,
    /// authoring-format-specific path (parse, decode, bake). Run only on a cache miss. An
    /// `Err` is a malformed source and propagates out of [`load_or_compile`].
    fn compile(source: &[u8]) -> Result<Self, String>;
}

/// Fixed header preceding the `bincode` payload: `magic(8) + version(4) + source_hash(8) +
/// payload_len(8)`, all little-endian. Hand-encoded (not `bincode`) so it stays stable and
/// inspectable independent of the payload's format version.
const HEADER_LEN: usize = 8 + 4 + 8 + 8;

/// Load `id`'s compiled asset, compiling + caching it on a miss.
///
/// `source` is the authored bytes (the `.bbmodel` text, the mod file, …); they key the
/// cache, so an unchanged source and format version reuse the cached file while any change
/// rebuilds it. `id` is the stable file stem (e.g. a mob key like `"owl"` → `owl.llmob`).
///
/// Only a *compile* failure is an `Err`; cache IO failures are logged and the compiled
/// asset is returned regardless (see module docs).
pub fn load_or_compile<A: CompiledAsset>(id: &str, source: &[u8]) -> Result<A, String> {
    load_or_compile_in::<A>(&cache_root(), id, source)
}

/// [`load_or_compile`] against an explicit cache `root` — the filesystem-touching core,
/// split out so tests can drive it against a temp dir instead of the real cache.
fn load_or_compile_in<A: CompiledAsset>(root: &Path, id: &str, source: &[u8]) -> Result<A, String> {
    let hash = hash_source(source);
    let path = asset_path::<A>(root, id);

    // Fast path: a present, valid entry for this exact source + format.
    if let Ok(bytes) = std::fs::read(&path) {
        if let Some(asset) = decode::<A>(&bytes, hash) {
            return Ok(asset);
        }
        // Present but stale/corrupt — fall through and rebuild (overwriting it).
    }

    // Slow path: compile from source, then best-effort cache the result for next time.
    let asset = A::compile(source)?;
    match encode::<A>(hash, &asset) {
        Ok(bytes) => {
            if let Err(e) = atomic_write(&path, &bytes) {
                log::warn!("asset cache write failed for {}: {e}", path.display());
            }
        }
        Err(e) => log::warn!("asset cache encode failed for {}: {e}", path.display()),
    }
    Ok(asset)
}

/// Serialise `asset` into a full cache file (header + `bincode` payload).
fn encode<A: CompiledAsset>(source_hash: u64, asset: &A) -> Result<Vec<u8>, String> {
    let payload = bincode::serialize(asset).map_err(|e| format!("bincode: {e}"))?;
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(&A::MAGIC);
    out.extend_from_slice(&A::FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&source_hash.to_le_bytes());
    out.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    out.extend_from_slice(&payload);
    Ok(out)
}

/// Validate a cache file against the expected kind/version/`source_hash` and deserialise
/// its payload. Returns `None` for any mismatch or corruption — the caller then rebuilds.
fn decode<A: CompiledAsset>(bytes: &[u8], source_hash: u64) -> Option<A> {
    if bytes.len() < HEADER_LEN || bytes[..8] != A::MAGIC[..] {
        return None;
    }
    let version = u32::from_le_bytes(bytes[8..12].try_into().ok()?);
    let hash = u64::from_le_bytes(bytes[12..20].try_into().ok()?);
    let payload_len = u64::from_le_bytes(bytes[20..28].try_into().ok()?) as usize;
    if version != A::FORMAT_VERSION || hash != source_hash {
        return None;
    }
    let payload = bytes.get(HEADER_LEN..)?;
    if payload.len() != payload_len {
        return None; // truncated or trailing garbage
    }
    bincode::deserialize(payload).ok()
}

/// FNV-1a 64-bit hash of `bytes`. Kept in-tree (no hashing dependency, in the spirit of the
/// loader's own base64 decoder) and fully deterministic across platforms and toolchains —
/// unlike the std `DefaultHasher`, whose algorithm is unspecified — so a file written by one
/// build validates under another.
fn hash_source(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = OFFSET;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(PRIME);
    }
    h
}

/// Where a kind's cache file for `id` lives: `<root>/<SUBDIR>/<id>.<EXTENSION>`.
fn asset_path<A: CompiledAsset>(root: &Path, id: &str) -> PathBuf {
    root.join(A::SUBDIR).join(format!("{id}.{}", A::EXTENSION))
}

/// The cache root: `<OS cache dir>/llamacraft` (e.g. `~/.cache/llamacraft` on Linux), the
/// conventional home for regenerable derived data and a sibling of the save data under the
/// OS data dir. Falls back to a hidden cwd dir if no cache dir resolves (mirrors
/// [`crate::save`]'s data-dir fallback).
fn cache_root() -> PathBuf {
    directories::ProjectDirs::from("", "", "llamacraft")
        .map(|d| d.cache_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".llamacraft-cache"))
}

/// Atomically write `bytes` to `path`: create the dir, write a uniquely-named sibling temp,
/// then rename it over `path`. The rename is atomic on a single filesystem, so a reader (or
/// a crash) never sees a half-written file. The temp name is disambiguated by pid + a
/// process-local counter so concurrent writers (even of the same asset) can't collide.
fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("asset");
    let tmp = path.with_file_name(format!("{name}.tmp.{}.{n}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    /// A throwaway asset whose "compiler" just mirrors the source bytes, so a compiled
    /// value is distinguishable from a tampered cached value (used to prove cache hits
    /// don't recompile). Exercises the same machinery as a real [`CompiledAsset`].
    #[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
    struct Dummy {
        tag: String,
        data: Vec<u32>,
    }

    impl CompiledAsset for Dummy {
        const MAGIC: [u8; 8] = *b"LLTEST\0\0";
        const FORMAT_VERSION: u32 = 1;
        const SUBDIR: &'static str = "test";
        const EXTENSION: &'static str = "lltest";
        fn compile(source: &[u8]) -> Result<Self, String> {
            if source == b"bad" {
                return Err("deliberate compile failure".into());
            }
            Ok(Dummy {
                tag: String::from_utf8_lossy(source).into_owned(),
                data: source.iter().map(|&b| b as u32).collect(),
            })
        }
    }

    /// A unique, fresh temp dir per call (cargo runs tests in parallel; no `Date`/random
    /// needed — pid + a counter suffice).
    fn temp_root() -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("llamacraft-asset-cache-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn compiles_writes_then_a_hit_reads_the_file_instead_of_recompiling() {
        let root = temp_root();
        let src = b"owl-source-bytes";

        // First load compiles and writes the cache file.
        let first = load_or_compile_in::<Dummy>(&root, "owl", src).unwrap();
        assert_eq!(first, Dummy::compile(src).unwrap());
        let path = asset_path::<Dummy>(&root, "owl");
        assert!(path.exists(), "cache file written on first load");

        // Tamper the payload (keeping a valid header + the SAME source hash). A second
        // load must return the tampered value — proving it read the file rather than
        // recompiling (a recompile would reproduce `first`).
        let sentinel = Dummy {
            tag: "from-cache".into(),
            data: vec![7, 8, 9],
        };
        atomic_write(
            &path,
            &encode::<Dummy>(hash_source(src), &sentinel).unwrap(),
        )
        .unwrap();
        let second = load_or_compile_in::<Dummy>(&root, "owl", src).unwrap();
        assert_eq!(second, sentinel, "a cache hit must not recompile");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_source_change_invalidates_and_recompiles() {
        let root = temp_root();
        let a = load_or_compile_in::<Dummy>(&root, "m", b"source-a").unwrap();
        // Same id, different source → different hash → miss → recompile (overwrites).
        let b = load_or_compile_in::<Dummy>(&root, "m", b"source-b").unwrap();
        assert_eq!(a, Dummy::compile(b"source-a").unwrap());
        assert_eq!(b, Dummy::compile(b"source-b").unwrap());
        assert_ne!(a, b);
        // The on-disk entry now matches source-b, not source-a.
        assert!(decode::<Dummy>(
            &std::fs::read(asset_path::<Dummy>(&root, "m")).unwrap(),
            hash_source(b"source-b")
        )
        .is_some());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_corrupt_cache_file_is_rebuilt_not_fatal() {
        let root = temp_root();
        let path = asset_path::<Dummy>(&root, "x");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"not a valid cache file").unwrap();
        // Garbage in the slot must not be fatal: it recompiles and rewrites a valid file.
        let got = load_or_compile_in::<Dummy>(&root, "x", b"hello").unwrap();
        assert_eq!(got, Dummy::compile(b"hello").unwrap());
        assert!(decode::<Dummy>(&std::fs::read(&path).unwrap(), hash_source(b"hello")).is_some());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn compile_failure_propagates() {
        let root = temp_root();
        assert!(load_or_compile_in::<Dummy>(&root, "x", b"bad").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn decode_rejects_wrong_magic_version_hash_and_truncation() {
        let hash = hash_source(b"src");
        let good = encode::<Dummy>(hash, &Dummy::compile(b"src").unwrap()).unwrap();
        assert!(
            decode::<Dummy>(&good, hash).is_some(),
            "a faithful file decodes"
        );

        // Wrong source hash (the source changed underneath it).
        assert!(decode::<Dummy>(&good, hash ^ 1).is_none());

        // Wrong magic (a file of another kind / not ours).
        let mut bad_magic = good.clone();
        bad_magic[0] ^= 0xFF;
        assert!(decode::<Dummy>(&bad_magic, hash).is_none());

        // Wrong format version (an older/newer compiled layout).
        let mut bad_ver = good.clone();
        bad_ver[8] ^= 0xFF;
        assert!(decode::<Dummy>(&bad_ver, hash).is_none());

        // Truncated payload, and a runt shorter than the header.
        assert!(decode::<Dummy>(&good[..good.len() - 1], hash).is_none());
        assert!(decode::<Dummy>(&good[..4], hash).is_none());
    }

    #[test]
    fn hash_is_stable_and_distinguishes_sources() {
        assert_eq!(hash_source(b"abc"), hash_source(b"abc"), "deterministic");
        assert_ne!(hash_source(b"abc"), hash_source(b"abd"));
        assert_ne!(hash_source(b""), hash_source(b"\0"));
    }
}
