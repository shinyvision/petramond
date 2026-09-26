//! Whole-file writes a crash cannot tear: the bytes go to a temporary sibling
//! first and only a complete file ever appears under the real name.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Whether a write must survive power loss, or only never be seen torn.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Durability {
    /// The file and its directory entry are flushed to disk before returning.
    Synced,
    /// Atomic against a process crash only. For data that can be rebuilt.
    Unsynced,
}

/// Replace `path` with `bytes`, durably.
pub fn replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    replace_with(path, Durability::Synced, |file| file.write_all(bytes))
}

/// Replace `path` with whatever `write` produces. A failure leaves the
/// previous file untouched.
pub fn replace_with(
    path: &Path,
    durability: Durability,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<()> {
    let temporary = temporary_sibling(path)?;
    let result = (|| {
        let mut file = File::create(&temporary)?;
        write(&mut file)?;
        if durability == Durability::Synced {
            file.sync_all()?;
        }
        drop(file);
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result?;
    match durability {
        Durability::Synced => sync_parent(path),
        Durability::Unsynced => Ok(()),
    }
}

/// Create `path` durably from whatever `write` produces, never overwriting:
/// an existing `path` fails with [`io::ErrorKind::AlreadyExists`] and keeps
/// its content.
pub fn publish_new(path: &Path, write: impl FnOnce(&mut File) -> io::Result<()>) -> io::Result<()> {
    let temporary = temporary_sibling(path)?;
    let result = (|| {
        let mut file = File::create(&temporary)?;
        write(&mut file)?;
        file.sync_all()?;
        // A hard link appears atomically and refuses an existing name.
        fs::hard_link(&temporary, path)
    })();
    let _ = fs::remove_file(&temporary);
    result?;
    sync_parent(path)
}

/// Flush `dir`'s entries (a rename, a link, a removal) to disk.
pub fn sync_dir(dir: &Path) -> io::Result<()> {
    // Directory handles only sync on unix; elsewhere the rename is what the
    // platform offers.
    #[cfg(unix)]
    File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

fn sync_parent(path: &Path) -> io::Result<()> {
    match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => sync_dir(dir),
        _ => sync_dir(Path::new(".")),
    }
}

/// A hidden name beside `path` (same filesystem, so the rename or link is
/// atomic), unique among concurrent writers.
fn temporary_sibling(path: &Path) -> io::Result<PathBuf> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path names no file"))?;
    let mut temporary = std::ffi::OsString::from(".");
    temporary.push(name);
    temporary.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    Ok(path.with_file_name(temporary))
}

#[cfg(test)]
mod tests;
