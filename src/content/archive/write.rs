//! The one archive writer: a pack folder as the zip `make addons` ships.
//!
//! Identical files always make identical bytes: entries sorted by name, one
//! fixed timestamp, one compression level, no directory entries and no extra
//! fields. It writes nothing the reader in the parent module would refuse
//! for its shape (no links, no Zip64, no encryption), so `check` judges only
//! the content.

use std::io::Write;
use std::path::{Path, PathBuf};

use super::{CENTRAL_HEADER, END_HEADER, LOCAL_HEADER};
use super::{MANIFEST, MAX_ENTRIES, MAX_ENTRY_BYTES, MAX_TOTAL_BYTES};

/// 1980-01-01 00:00, the earliest time a zip can say.
const DOS_DATE: u16 = (1 << 5) | 1;
const DOS_TIME: u16 = 0;
const VERSION: u16 = 20;
/// Made on unix, so the external attributes carry a plain file's mode.
const MADE_BY: u16 = (3 << 8) | VERSION;
const FILE_MODE: u32 = 0o100_644;
const UTF8_NAMES: u16 = 1 << 11;
const DEFLATE: u16 = 8;

/// Zip the pack folder `dir` (its CONTENTS, `pack.json` at the root), with
/// `wasm`, when given, as its `mod.wasm`.
pub fn pack(dir: &Path, wasm: Option<&Path>) -> Result<Vec<u8>, String> {
    let mut files = Vec::new();
    collect(dir, "", &mut files)?;
    if let Some(wasm) = wasm {
        if files.iter().any(|(name, _)| name == "mod.wasm") {
            return Err(format!(
                "{} already has a mod.wasm; drop it or --wasm",
                dir.display()
            ));
        }
        files.push(("mod.wasm".to_owned(), wasm.to_path_buf()));
    }
    if !files.iter().any(|(name, _)| name == MANIFEST) {
        return Err(format!("{} has no pack.json", dir.display()));
    }
    if files.len() > MAX_ENTRIES {
        return Err(format!(
            "{} holds more than {MAX_ENTRIES} files",
            dir.display()
        ));
    }
    files.sort();

    let mut out = Vec::new();
    let mut central = Vec::new();
    let mut total = 0u64;
    for (name, path) in &files {
        let data =
            std::fs::read(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
        total += data.len() as u64;
        if data.len() as u64 > MAX_ENTRY_BYTES || total > MAX_TOTAL_BYTES {
            return Err(format!(
                "{} is past the archive limits (16 MB a file, 64 MB in all)",
                path.display()
            ));
        }
        let mut encoder =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::best());
        encoder.write_all(&data).map_err(|e| e.to_string())?;
        let packed = encoder.finish().map_err(|e| e.to_string())?;
        let mut crc = flate2::Crc::new();
        crc.update(&data);
        let flags = if name.is_ascii() { 0 } else { UTF8_NAMES };
        let offset = out.len() as u32;
        let shared = |b: &mut Vec<u8>| {
            put16(b, VERSION);
            put16(b, flags);
            put16(b, DEFLATE);
            put16(b, DOS_TIME);
            put16(b, DOS_DATE);
            put32(b, crc.sum());
            put32(b, packed.len() as u32);
            put32(b, data.len() as u32);
            put16(b, name.len() as u16);
            put16(b, 0);
        };
        put32(&mut out, LOCAL_HEADER);
        shared(&mut out);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&packed);

        put32(&mut central, CENTRAL_HEADER);
        put16(&mut central, MADE_BY);
        shared(&mut central);
        put16(&mut central, 0);
        put16(&mut central, 0);
        put16(&mut central, 0);
        put32(&mut central, FILE_MODE << 16);
        put32(&mut central, offset);
        central.extend_from_slice(name.as_bytes());
    }
    let offset = out.len() as u32;
    out.extend_from_slice(&central);
    put32(&mut out, END_HEADER);
    put16(&mut out, 0);
    put16(&mut out, 0);
    put16(&mut out, files.len() as u16);
    put16(&mut out, files.len() as u16);
    put32(&mut out, central.len() as u32);
    put32(&mut out, offset);
    put16(&mut out, 0);
    Ok(out)
}

/// Every file under `dir` as `(archive name, path)`. A link is refused, not
/// followed: the archive must hold the pack's real files.
fn collect(dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("could not read {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("could not read {}: {e}", dir.display()))?;
        let path = entry.path();
        let Some(leaf) = entry.file_name().to_str().map(str::to_owned) else {
            return Err(format!("{} has a name that is not UTF-8", path.display()));
        };
        if matches!(leaf.as_str(), ".DS_Store" | "Thumbs.db") {
            continue;
        }
        let name = format!("{prefix}{leaf}");
        let kind = entry
            .file_type()
            .map_err(|e| format!("could not read {}: {e}", path.display()))?;
        if kind.is_symlink() {
            return Err(format!(
                "{} is a symbolic link; a pack ships real files",
                path.display()
            ));
        }
        if kind.is_dir() {
            collect(&path, &format!("{name}/"), out)?;
        } else {
            out.push((name, path));
        }
    }
    Ok(())
}

fn put16(b: &mut Vec<u8>, v: u16) {
    b.extend_from_slice(&v.to_le_bytes());
}

fn put32(b: &mut Vec<u8>, v: u32) {
    b.extend_from_slice(&v.to_le_bytes());
}
