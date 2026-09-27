//! Per-process test data dirs and per-test scratch dirs under the temp dir,
//! reaped once their owning process is gone.

use std::path::{Path, PathBuf};

/// Every test-owned entry under the temp dir is `petramond-test-<kind>-<pid>…`,
/// so one sweep can tell whose it is and reap it once that process is gone.
const TEST_DIR_PREFIX: &str = "petramond-test-";

/// The data dir a test process points `PETRAMOND_DATA_DIR` at: one per
/// process under the temp dir, so saves, storage and the module cache never
/// touch the real user dir. Removed when the process exits normally; a
/// crashed process's is reaped by the next test process.
pub fn test_process_data_dir() -> PathBuf {
    static AT_EXIT: std::sync::Once = std::sync::Once::new();
    sweep_exited_test_dirs();
    AT_EXIT.call_once(|| {
        // SAFETY: `atexit` is the C runtime's; the callback is a plain
        // `extern "C" fn` that never unwinds.
        unsafe { atexit(remove_test_process_data_dir) };
    });
    own_test_process_data_dir()
}

fn own_test_process_data_dir() -> PathBuf {
    std::env::temp_dir().join(format!("{TEST_DIR_PREFIX}data-{}", std::process::id()))
}

extern "C" {
    fn atexit(callback: extern "C" fn()) -> std::ffi::c_int;
}

extern "C" fn remove_test_process_data_dir() {
    remove_test_dir(&own_test_process_data_dir());
}

/// A fresh, empty scratch dir for one test, removed on drop — which also runs
/// when the test panics. A process that dies without unwinding leaves it to
/// the next test process's sweep.
pub struct TestScratchDir(PathBuf);

impl TestScratchDir {
    pub fn new(tag: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        sweep_exited_test_dirs();
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "{TEST_DIR_PREFIX}scratch-{}-{tag}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("test scratch dir");
        Self(path)
    }
}

impl std::ops::Deref for TestScratchDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for TestScratchDir {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestScratchDir {
    fn drop(&mut self) {
        remove_test_dir(&self.0);
    }
}

fn remove_test_dir(path: &Path) {
    if std::fs::remove_dir_all(path).is_err() {
        make_writable(path);
        let _ = std::fs::remove_dir_all(path);
    }
}

/// A test that locks a dir read-only and fails before unlocking it would
/// otherwise leave it behind for good.
#[cfg(unix)]
fn make_writable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return;
    };
    if !meta.is_dir() {
        return;
    }
    let mode = meta.permissions().mode() | 0o700;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    for entry in std::fs::read_dir(path).into_iter().flatten().flatten() {
        make_writable(&entry.path());
    }
}

#[cfg(not(unix))]
fn make_writable(_path: &Path) {}

fn sweep_exited_test_dirs() {
    static SWEPT: std::sync::Once = std::sync::Once::new();
    SWEPT.call_once(|| remove_exited_test_dirs(&std::env::temp_dir()));
}

/// The pid in `petramond-test-<kind>-<pid>…`.
fn test_dir_owner(name: &str) -> Option<u32> {
    let (_kind, rest) = name.strip_prefix(TEST_DIR_PREFIX)?.split_once('-')?;
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

#[cfg(target_os = "linux")]
fn remove_exited_test_dirs(tmp: &Path) {
    let Ok(entries) = std::fs::read_dir(tmp) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(test_dir_owner) else {
            continue;
        };
        if pid != std::process::id() && !Path::new(&format!("/proc/{pid}")).exists() {
            remove_test_dir(&entry.path());
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn remove_exited_test_dirs(_tmp: &Path) {}
