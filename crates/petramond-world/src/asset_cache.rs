use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::de::DeserializeOwned;
use serde::Serialize;

pub trait CompiledAsset: Serialize + DeserializeOwned + Sized {
    const MAGIC: [u8; 8];

    const FORMAT_VERSION: u32;

    const SUBDIR: &'static str;

    const EXTENSION: &'static str;

    fn compile(source: &[u8]) -> Result<Self, String>;
}

const HEADER_LEN: usize = 8 + 4 + 8 + 8;

pub fn load_or_compile<A: CompiledAsset>(id: &str, source: &[u8]) -> Result<A, String> {
    load_or_compile_in::<A>(&cache_root(), id, source)
}

fn load_or_compile_in<A: CompiledAsset>(root: &Path, id: &str, source: &[u8]) -> Result<A, String> {
    let hash = hash_source(source);
    let path = asset_path::<A>(root, id);

    if let Ok(bytes) = std::fs::read(&path) {
        if let Some(asset) = decode::<A>(&bytes, hash) {
            return Ok(asset);
        }
    }

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
        return None;
    }
    bincode::deserialize(payload).ok()
}

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

fn asset_path<A: CompiledAsset>(root: &Path, id: &str) -> PathBuf {
    root.join(A::SUBDIR).join(format!("{id}.{}", A::EXTENSION))
}

fn cache_root() -> PathBuf {
    directories::ProjectDirs::from("", "", "petramond")
        .map(|d| d.cache_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".petramond-cache"))
}

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

    #[test]
    fn compiles_writes_then_a_hit_reads_the_file_instead_of_recompiling() {
        let root = petramond_util::test_dirs::TestScratchDir::new("asset-cache");
        let src = b"owl-source-bytes";

        let first = load_or_compile_in::<Dummy>(&root, "owl", src).unwrap();
        assert_eq!(first, Dummy::compile(src).unwrap());
        let path = asset_path::<Dummy>(&root, "owl");
        assert!(path.exists(), "cache file written on first load");

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
    }

    #[test]
    fn a_source_change_invalidates_and_recompiles() {
        let root = petramond_util::test_dirs::TestScratchDir::new("asset-cache");
        let a = load_or_compile_in::<Dummy>(&root, "m", b"source-a").unwrap();
        let b = load_or_compile_in::<Dummy>(&root, "m", b"source-b").unwrap();
        assert_eq!(a, Dummy::compile(b"source-a").unwrap());
        assert_eq!(b, Dummy::compile(b"source-b").unwrap());
        assert_ne!(a, b);
        assert!(decode::<Dummy>(
            &std::fs::read(asset_path::<Dummy>(&root, "m")).unwrap(),
            hash_source(b"source-b")
        )
        .is_some());
    }

    #[test]
    fn a_corrupt_cache_file_is_rebuilt_not_fatal() {
        let root = petramond_util::test_dirs::TestScratchDir::new("asset-cache");
        let path = asset_path::<Dummy>(&root, "x");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"not a valid cache file").unwrap();
        let got = load_or_compile_in::<Dummy>(&root, "x", b"hello").unwrap();
        assert_eq!(got, Dummy::compile(b"hello").unwrap());
        assert!(decode::<Dummy>(&std::fs::read(&path).unwrap(), hash_source(b"hello")).is_some());
    }

    #[test]
    fn compile_failure_propagates() {
        let root = petramond_util::test_dirs::TestScratchDir::new("asset-cache");
        assert!(load_or_compile_in::<Dummy>(&root, "x", b"bad").is_err());
    }

    #[test]
    fn decode_rejects_wrong_magic_version_hash_and_truncation() {
        let hash = hash_source(b"src");
        let good = encode::<Dummy>(hash, &Dummy::compile(b"src").unwrap()).unwrap();
        assert!(
            decode::<Dummy>(&good, hash).is_some(),
            "a faithful file decodes"
        );

        assert!(decode::<Dummy>(&good, hash ^ 1).is_none());

        let mut bad_magic = good.clone();
        bad_magic[0] ^= 0xFF;
        assert!(decode::<Dummy>(&bad_magic, hash).is_none());

        let mut bad_ver = good.clone();
        bad_ver[8] ^= 0xFF;
        assert!(decode::<Dummy>(&bad_ver, hash).is_none());

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
