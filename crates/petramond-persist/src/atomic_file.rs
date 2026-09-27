use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Durability {
    Synced,
    Unsynced,
}

pub fn replace(path: &Path, bytes: &[u8]) -> io::Result<()> {
    replace_with(path, Durability::Synced, |file| file.write_all(bytes))
}

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

pub fn publish_new(path: &Path, write: impl FnOnce(&mut File) -> io::Result<()>) -> io::Result<()> {
    let temporary = temporary_sibling(path)?;
    let result = (|| {
        let mut file = File::create(&temporary)?;
        write(&mut file)?;
        file.sync_all()?;
        fs::hard_link(&temporary, path)
    })();
    let _ = fs::remove_file(&temporary);
    result?;
    sync_parent(path)
}

pub fn sync_dir(dir: &Path) -> io::Result<()> {
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
