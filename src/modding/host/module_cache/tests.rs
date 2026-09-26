use super::*;

/// The per-process test data root the app tests also use — set, never
/// removed, identical value from every setter, so parallel tests can't race
/// each other onto the real user dir.
fn isolated_data_dir() -> PathBuf {
    let data = std::env::temp_dir().join(format!("petramond-test-data-{}", std::process::id()));
    std::env::set_var("PETRAMOND_DATA_DIR", &data);
    std::fs::create_dir_all(&data).unwrap();
    data
}

/// A fresh source file holding `wasm` (WAT text compiles like binary wasm
/// through the dev-build's `wat` feature), with no cache entry left from an
/// earlier run.
fn fresh_source(name: &str, wasm: &[u8]) -> (PathBuf, CacheEntry) {
    let data = isolated_data_dir();
    let source = data.join(name);
    std::fs::write(&source, wasm).unwrap();
    let entry = CacheEntry::for_source(&source, wasm).expect("disk cache enabled");
    let _ = std::fs::remove_file(&entry.artifact);
    let _ = std::fs::remove_file(&entry.manifest_path);
    (source, entry)
}

/// The disk artifact round-trips: a second process-cache miss for the same
/// bytes loads the verified artifact instead of recompiling, and a content
/// change misses to a fresh compile while GC drops the stale artifact (and
/// its manifest) of the same path.
#[test]
fn artifact_roundtrip_and_stale_gc() {
    let wasm_a = b"(module (memory (export \"memory\") 1))".to_vec();
    let wasm_b = b"(module (memory (export \"memory\") 2) (func (export \"f\")))".to_vec();
    let (source, first) = fresh_source("module-cache-guest.wasm", &wasm_a);

    assert_eq!(load_module_traced(&source).unwrap().1, Origin::Compiled);
    assert!(first.artifact.exists(), "compile stores the artifact");
    assert!(first.manifest_path.exists(), "and its manifest");
    assert_eq!(load_module_traced(&source).unwrap().1, Origin::Cache);

    std::fs::write(&source, &wasm_b).unwrap();
    let second = CacheEntry::for_source(&source, &wasm_b).unwrap();
    assert_ne!(
        first.artifact, second.artifact,
        "content change changes the key"
    );
    assert_eq!(load_module_traced(&source).unwrap().1, Origin::Compiled);
    assert!(
        second.artifact.exists(),
        "recompile stores the new artifact"
    );
    assert!(
        !first.artifact.exists(),
        "stale artifact of the same path is GC'd"
    );
    assert!(!first.manifest_path.exists(), "with its manifest");
}

/// A tampered or torn artifact is never deserialized: its bytes no longer
/// match the manifest's keyed hash, so the load recompiles and rewrites a
/// valid entry.
#[test]
fn tampered_artifact_recompiles() {
    let wasm = b"(module (memory (export \"memory\") 1) (func (export \"t\")))".to_vec();
    let (source, entry) = fresh_source("module-cache-tamper.wasm", &wasm);
    load_module(&source).unwrap();

    let mut bytes = std::fs::read(&entry.artifact).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xff;
    std::fs::write(&entry.artifact, &bytes).unwrap();
    assert!(matches!(entry.load_verified(), Err(Rejection::Invalid(_))));
    assert_eq!(load_module_traced(&source).unwrap().1, Origin::Compiled);
    assert_eq!(load_module_traced(&source).unwrap().1, Origin::Cache);

    // Torn write: the artifact is shorter than the manifest says.
    let bytes = std::fs::read(&entry.artifact).unwrap();
    std::fs::write(&entry.artifact, &bytes[..bytes.len() / 2]).unwrap();
    assert_eq!(load_module_traced(&source).unwrap().1, Origin::Compiled);
}

/// A manifest written for another engine build or mod ABI does not vouch for
/// its artifact, even when the artifact bytes themselves are intact.
#[test]
fn manifest_bound_to_engine_and_abi() {
    let wasm = b"(module (memory (export \"memory\") 1) (func (export \"e\")))".to_vec();
    let (source, entry) = fresh_source("module-cache-engine.wasm", &wasm);
    load_module(&source).unwrap();

    let text = std::fs::read_to_string(&entry.manifest_path).unwrap();
    let mut manifest = Manifest::parse(&text).expect("the stored manifest parses");
    assert_eq!(manifest.render(), text, "render/parse round-trips");
    manifest.abi ^= 1;
    std::fs::write(&entry.manifest_path, manifest.render()).unwrap();
    assert!(matches!(entry.load_verified(), Err(Rejection::Invalid(_))));
    assert_eq!(load_module_traced(&source).unwrap().1, Origin::Compiled);

    let mut manifest = Manifest::parse(&std::fs::read_to_string(&entry.manifest_path).unwrap())
        .expect("rewritten manifest parses");
    manifest.engine[0] ^= 1;
    std::fs::write(&entry.manifest_path, manifest.render()).unwrap();
    assert_eq!(load_module_traced(&source).unwrap().1, Origin::Compiled);

    std::fs::write(&entry.manifest_path, "not a manifest").unwrap();
    assert_eq!(load_module_traced(&source).unwrap().1, Origin::Compiled);
}

/// An artifact with no manifest (a crash between the two writes, or a file
/// dropped into the cache by hand) is treated as absent and recompiled.
#[test]
fn artifact_without_manifest_is_not_loaded() {
    let wasm = b"(module (memory (export \"memory\") 1) (func (export \"m\")))".to_vec();
    let (source, entry) = fresh_source("module-cache-orphan.wasm", &wasm);
    load_module(&source).unwrap();
    std::fs::remove_file(&entry.manifest_path).unwrap();
    assert!(matches!(entry.load_verified(), Err(Rejection::Absent)));
    assert_eq!(load_module_traced(&source).unwrap().1, Origin::Compiled);
}

#[test]
fn engine_fingerprint_is_stable_within_a_build() {
    assert_eq!(engine_fingerprint(), engine_fingerprint());
    assert_eq!(
        unhex(&hex(&engine_fingerprint())),
        Some(engine_fingerprint())
    );
    assert_eq!(unhex("zz"), None);
}
