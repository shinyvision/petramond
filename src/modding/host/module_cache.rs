//! Compiled-module cache: a process-wide per-path map backed by a disk cache
//! of serialized cranelift artifacts (`<data>/modcache/*.cwasm`), so opening a
//! world never pays a wasm compile for an unchanged module. A cold compile is
//! ~300 ms per bundled mod; a warm artifact deserializes in ~1 ms.
//!
//! Cache key = (wasm path, wasm content + length, engine compatibility hash):
//! rebuilding a mod or upgrading wasmtime/config misses and recompiles, and a
//! stale artifact for the same path is garbage-collected on the next store.
//! Deserialization failures (however they happen) fall back to compiling.
//!
//! Concurrency: each path owns a `OnceLock` slot, so [`prewarm`] can compile
//! many modules on background threads while a session load blocks only on the
//! module it actually needs.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::Instant;

use wasmtime::Module;

use super::engine;

type Slot = Arc<OnceLock<Result<Module, String>>>;

static CACHE: LazyLock<Mutex<HashMap<PathBuf, Slot>>> = LazyLock::new(Default::default);

/// Compile (or fetch the cached compilation of) the module at `path`.
/// Successes are cached per path for the process lifetime; a failure is
/// reported to every concurrent waiter but retried by later calls (the file
/// may have been rebuilt in place).
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

/// Warm the module cache for `paths` on background threads. Returns
/// immediately; a later [`module_for`] for one of these paths blocks on its
/// in-flight slot instead of duplicating work, so callers about to load a
/// list of modules get parallel cold compiles for free. Already-cached paths
/// spawn nothing.
pub(crate) fn prewarm(paths: impl IntoIterator<Item = PathBuf>) {
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
            return; // thread exhaustion: sessions still compile on demand
        }
    }
}

fn load_module(path: &Path) -> Result<Module, String> {
    let t = Instant::now();
    let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let artifact = artifact_path(path, &bytes);
    if let Some(artifact) = &artifact {
        if artifact.exists() {
            // SAFETY: the artifact was serialized by us into the user's own
            // data dir — the same trust domain as the game binary and saves —
            // and wasmtime rejects artifacts from an incompatible build.
            match unsafe { Module::deserialize_file(engine(), artifact) } {
                Ok(module) => {
                    log::debug!(
                        target: "petramond::modding::perf",
                        "loaded precompiled {} in {:.1} ms",
                        path.display(),
                        t.elapsed().as_secs_f64() * 1e3
                    );
                    return Ok(module);
                }
                Err(e) => log::debug!(
                    "discarding stale precompiled artifact for {}: {e:#}",
                    path.display()
                ),
            }
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
    if let Some(artifact) = &artifact {
        store_artifact(&module, artifact);
    }
    Ok(module)
}

/// The disk-cache file for this (source path, content, engine config), or
/// `None` when the disk cache is unusable. Unit tests skip the disk cache
/// unless they isolate the data dir — fixture guests must not litter (or
/// read) the developer's real `modcache/`.
fn artifact_path(path: &Path, bytes: &[u8]) -> Option<PathBuf> {
    if cfg!(test) && std::env::var_os("PETRAMOND_DATA_DIR").is_none() {
        return None;
    }
    let dir = crate::save::base_data_dir().join("modcache");
    std::fs::create_dir_all(&dir).ok()?;
    let mut compat = std::hash::DefaultHasher::new();
    engine().precompile_compatibility_hash().hash(&mut compat);
    Some(dir.join(format!(
        "{:016x}-{:016x}-{:x}-{:016x}.cwasm",
        fnv1a64(path.to_string_lossy().as_bytes()),
        fnv1a64(bytes),
        bytes.len(),
        compat.finish()
    )))
}

/// Serialize `module` to its cache file (atomically, via a temp file), then
/// drop other artifacts of the same source path — a rebuilt mod must not
/// accumulate one orphan per build. Failures are ignored: the cache is an
/// accelerator, never a correctness dependency.
fn store_artifact(module: &Module, artifact: &Path) {
    let Ok(bytes) = module.serialize() else {
        return;
    };
    let tmp = artifact.with_extension(format!("tmp{}", std::process::id()));
    if std::fs::write(&tmp, bytes).is_err() || std::fs::rename(&tmp, artifact).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    let (Some(dir), Some(name)) = (artifact.parent(), artifact.file_name()) else {
        return;
    };
    let Some(path_prefix) = name.to_str().and_then(|n| n.split('-').next()) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let stale = entry.file_name() != name
            && entry
                .file_name()
                .to_str()
                .is_some_and(|n| n.starts_with(path_prefix) && n[path_prefix.len()..].starts_with('-'));
        if stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// FNV-1a — frozen; artifact names must be stable across builds. (A changed
/// hash would only orphan cache entries, but there is no reason to churn.)
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The disk artifact round-trips: a second process-cache miss for the same
    /// bytes loads the precompiled artifact instead of recompiling, and a
    /// content change misses to a fresh compile while GC drops the stale
    /// artifact of the same path.
    #[test]
    fn artifact_roundtrip_and_stale_gc() {
        // The per-process test data root the app tests also use — set, never
        // removed, identical value from every setter, so parallel tests can't
        // race each other onto the real user dir.
        let data = std::env::temp_dir().join(format!("petramond-test-data-{}", std::process::id()));
        std::env::set_var("PETRAMOND_DATA_DIR", &data);
        std::fs::create_dir_all(&data).unwrap();

        // WAT text compiles like binary wasm through the dev-build's `wat`
        // feature; two different bodies = two content hashes.
        let wasm_a = b"(module (memory (export \"memory\") 1))".to_vec();
        let wasm_b = b"(module (memory (export \"memory\") 2) (func (export \"f\")))".to_vec();
        let source = data.join("module-cache-guest.wasm");

        std::fs::write(&source, &wasm_a).unwrap();
        let first = artifact_path(&source, &wasm_a).expect("disk cache enabled");
        let _ = std::fs::remove_file(&first); // stale from an earlier run
        load_module(&source).unwrap();
        assert!(first.exists(), "compile stores the artifact");
        load_module(&source).unwrap(); // exercises the deserialize path
        assert!(first.exists());

        std::fs::write(&source, &wasm_b).unwrap();
        let second = artifact_path(&source, &wasm_b).unwrap();
        assert_ne!(first, second, "content change changes the key");
        load_module(&source).unwrap();
        assert!(second.exists(), "recompile stores the new artifact");
        assert!(!first.exists(), "stale artifact of the same path is GC'd");
    }
}
