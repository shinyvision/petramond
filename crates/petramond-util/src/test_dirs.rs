use std::path::{Path, PathBuf};

const TEST_DIR_PREFIX: &str = "petramond-test-";

pub fn test_process_data_dir() -> PathBuf {
    static AT_EXIT: std::sync::Once = std::sync::Once::new();
    sweep_exited_test_dirs();
    AT_EXIT.call_once(|| {
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
