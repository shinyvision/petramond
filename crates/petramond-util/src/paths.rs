//! User data directory resolution.

use std::path::PathBuf;

/// Base data dir: `~/.local/share/petramond` (Linux), `~/Library/Application
/// Support/petramond` (macOS), `%APPDATA%\petramond` (Windows). Falls back to
/// a hidden dir in the cwd if no home dir can be resolved. Also hosts the
/// user-installed mod pack root (`<data>/mods`).
/// `PETRAMOND_DATA_DIR` overrides it — tests point this at a temp dir so saves
/// and client-mod storage never touch the real user directory.
pub fn base_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("PETRAMOND_DATA_DIR") {
        return PathBuf::from(dir);
    }
    directories::ProjectDirs::from("", "", "petramond")
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".petramond"))
}
