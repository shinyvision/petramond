use std::collections::HashMap;
use std::hash::Hash;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::Instant;

use wasmtime::Module;

use super::engine;

type Slot = Arc<OnceLock<Result<Module, String>>>;

static CACHE: LazyLock<Mutex<HashMap<PathBuf, Slot>>> = LazyLock::new(Default::default);

const MANIFEST_MAGIC: &str = "petramond-modcache 1";

pub(in crate::modding) fn module_for(path: &Path) -> Result<Module, String> {
    let slot = {
        let mut cache = CACHE.lock().unwrap();
        Arc::clone(cache.entry(path.to_path_buf()).or_default())
    };
    let result = slot.get_or_init(|| load_module(path)).clone();
    if result.is_err() {
        let mut cache = CACHE.lock().unwrap();
        if cache.get(path).is_some_and(|s| Arc::ptr_eq(s, &slot)) {
            cache.remove(path);
        }
    }
    result
}

pub(in crate::modding) fn clear() {
    CACHE.lock().unwrap().clear();
}

pub fn prewarm(paths: impl IntoIterator<Item = PathBuf>) {
    for path in paths {
        {
            let cache = CACHE.lock().unwrap();
            if cache.get(&path).is_some_and(|slot| slot.get().is_some()) {
                continue;
            }
        }
        let spawned = std::thread::Builder::new()
            .name("mod-prewarm".into())
            .spawn(move || {
                let _ = module_for(&path);
            });
        if spawned.is_err() {
            return;
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Origin {
    Cache,
    Compiled,
}

fn load_module(path: &Path) -> Result<Module, String> {
    load_module_traced(path).map(|(module, _)| module)
}

fn load_module_traced(path: &Path) -> Result<(Module, Origin), String> {
    let t = Instant::now();
    let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let entry = CacheEntry::for_source(path, &bytes);
    if let Some(entry) = &entry {
        match entry.load_verified() {
            Ok(module) => {
                log::debug!(
                    target: "petramond::modding::perf",
                    "loaded precompiled {} in {:.1} ms",
                    path.display(),
                    t.elapsed().as_secs_f64() * 1e3
                );
                return Ok((module, Origin::Cache));
            }
            Err(Rejection::Absent) => {}
            Err(Rejection::Invalid(why)) => log::debug!(
                "discarding precompiled artifact for {}: {why}",
                path.display()
            ),
        }
    }
    let module =
        Module::new(engine(), &bytes).map_err(|e| format!("compile {}: {e:#}", path.display()))?;
    log::debug!(
        target: "petramond::modding::perf",
        "compiled {} ({} KiB) in {:.1} ms",
        path.display(),
        bytes.len() / 1024,
        t.elapsed().as_secs_f64() * 1e3
    );
    if let Some(entry) = &entry {
        entry.store(&module);
    }
    Ok((module, Origin::Compiled))
}

enum Rejection {
    Absent,
    Invalid(String),
}

struct CacheEntry {
    artifact: PathBuf,
    manifest_path: PathBuf,
    expected: Manifest,
    key: [u8; 32],
}

impl CacheEntry {
    fn for_source(path: &Path, wasm: &[u8]) -> Option<Self> {
        let dir = match std::env::var_os("PETRAMOND_MODCACHE_DIR") {
            Some(dir) => PathBuf::from(dir),
            None if cfg!(test) && std::env::var_os("PETRAMOND_DATA_DIR").is_none() => return None,
            None => petramond_util::paths::base_data_dir().join("modcache"),
        };
        std::fs::create_dir_all(&dir).ok()?;
        let key = install_key(&dir)?;
        let wasm_hash = blake3::hash(wasm);
        let path_hash = blake3::hash(path.to_string_lossy().as_bytes());
        let stem = format!(
            "{}-{}",
            &path_hash.to_hex()[..16],
            &wasm_hash.to_hex()[..32]
        );
        Some(Self {
            artifact: dir.join(format!("{stem}.cwasm")),
            manifest_path: dir.join(format!("{stem}.manifest")),
            expected: Manifest {
                wasm: *wasm_hash.as_bytes(),
                engine: engine_fingerprint(),
                abi: mod_api::ABI_VERSION.pack(),
                artifact: [0; 32],
                len: 0,
            },
            key,
        })
    }

    fn load_verified(&self) -> Result<Module, Rejection> {
        let Ok(text) = std::fs::read_to_string(&self.manifest_path) else {
            return Err(Rejection::Absent);
        };
        let manifest = Manifest::parse(&text)
            .ok_or_else(|| Rejection::Invalid("unreadable manifest".into()))?;
        if !manifest.binds_same_source(&self.expected) {
            return Err(Rejection::Invalid(
                "manifest is for another wasm, engine build or mod ABI".into(),
            ));
        }
        let bytes = std::fs::read(&self.artifact)
            .map_err(|e| Rejection::Invalid(format!("read artifact: {e}")))?;
        if bytes.len() as u64 != manifest.len
            || *blake3::keyed_hash(&self.key, &bytes).as_bytes() != manifest.artifact
        {
            return Err(Rejection::Invalid(
                "artifact bytes do not match their manifest hash".into(),
            ));
        }
        unsafe { Module::deserialize(engine(), &bytes) }
            .map_err(|e| Rejection::Invalid(format!("deserialize: {e:#}")))
    }

    fn store(&self, module: &Module) {
        let Ok(bytes) = module.serialize() else {
            return;
        };
        let manifest = Manifest {
            artifact: *blake3::keyed_hash(&self.key, &bytes).as_bytes(),
            len: bytes.len() as u64,
            ..self.expected
        };
        if write_durably(&self.artifact, &bytes).is_err()
            || write_durably(&self.manifest_path, manifest.render().as_bytes()).is_err()
        {
            return;
        }
        self.collect_stale();
    }

    fn collect_stale(&self) {
        let (Some(dir), Some(stem)) = (
            self.artifact.parent(),
            self.artifact.file_stem().and_then(|s| s.to_str()),
        ) else {
            return;
        };
        let Some(path_prefix) = stem.split('-').next() else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let same_source = name
                .strip_prefix(path_prefix)
                .is_some_and(|rest| rest.starts_with('-'));
            let current = Path::new(name).file_stem().and_then(|s| s.to_str()) == Some(stem);
            if same_source && !current {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Manifest {
    wasm: [u8; 32],
    engine: [u8; 32],
    abi: u32,
    artifact: [u8; 32],
    len: u64,
}

impl Manifest {
    fn render(&self) -> String {
        format!(
            "{MANIFEST_MAGIC}\nwasm {}\nengine {}\nabi {}\nartifact {}\nlen {}\n",
            hex(&self.wasm),
            hex(&self.engine),
            self.abi,
            hex(&self.artifact),
            self.len
        )
    }

    fn parse(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        if lines.next()? != MANIFEST_MAGIC {
            return None;
        }
        let wasm = unhex(manifest_field(&mut lines, "wasm")?)?;
        let engine = unhex(manifest_field(&mut lines, "engine")?)?;
        let abi = manifest_field(&mut lines, "abi")?.parse().ok()?;
        let artifact = unhex(manifest_field(&mut lines, "artifact")?)?;
        let len = manifest_field(&mut lines, "len")?.parse().ok()?;
        Some(Self {
            wasm,
            engine,
            abi,
            artifact,
            len,
        })
    }

    fn binds_same_source(&self, expected: &Manifest) -> bool {
        self.wasm == expected.wasm && self.engine == expected.engine && self.abi == expected.abi
    }
}

fn manifest_field<'a>(lines: &mut std::str::Lines<'a>, name: &str) -> Option<&'a str> {
    lines.next()?.strip_prefix(name)?.strip_prefix(' ')
}

fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 || !text.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

fn engine_fingerprint() -> [u8; 32] {
    struct Blake3Hasher(blake3::Hasher);
    impl std::hash::Hasher for Blake3Hasher {
        fn write(&mut self, bytes: &[u8]) {
            self.0.update(bytes);
        }
        fn finish(&self) -> u64 {
            let hash = self.0.finalize();
            u64::from_le_bytes(hash.as_bytes()[..8].try_into().expect("8 bytes"))
        }
    }
    let mut hasher = Blake3Hasher(blake3::Hasher::new());
    engine().precompile_compatibility_hash().hash(&mut hasher);
    *hasher.0.finalize().as_bytes()
}

fn install_key(dir: &Path) -> Option<[u8; 32]> {
    static KEY: Mutex<Option<(PathBuf, [u8; 32])>> = Mutex::new(None);
    let mut cached = KEY.lock().unwrap();
    if let Some((cached_dir, key)) = cached.as_ref() {
        if cached_dir == dir {
            return Some(*key);
        }
    }
    let path = dir.join("cache.key");
    let read = |path: &Path| -> Option<[u8; 32]> { std::fs::read(path).ok()?.try_into().ok() };
    let key = match read(&path) {
        Some(key) => key,
        None => {
            let mut fresh = [0u8; 32];
            getrandom::getrandom(&mut fresh).ok()?;
            write_durably(&path, &fresh).ok()?;
            read(&path)?
        }
    };
    *cached = Some((dir.to_path_buf(), key));
    Some(key)
}

fn write_durably(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    let written = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

#[cfg(test)]
mod tests;
