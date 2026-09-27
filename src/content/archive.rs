use std::collections::{BTreeSet, HashSet};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub const MAX_BYTES: usize = 20 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 4000;
pub const MAX_ENTRY_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;
const RATIO_FLOOR: u64 = 1024 * 1024;
const MAX_RATIO: u64 = 400;
const MANIFEST: &str = "pack.json";
const MANIFEST_MAX_BYTES: usize = 64 * 1024;
const ID_MAX: usize = 64;
const NAME_MAX: usize = 128;
const VERSION_MAX: usize = 32;

const LOCAL_HEADER: u32 = 0x0403_4b50;
const CENTRAL_HEADER: u32 = 0x0201_4b50;
const END_HEADER: u32 = 0x0605_4b50;
const SYMLINK_MODE: u32 = 0xA000;

const DAMAGED: &str = "That zip file is damaged or not a readable archive.";
const NO_MANIFEST: &str = "This archive has no pack.json. Zip the pack folder, or its contents.";
const TOO_BIG: &str = "Mod archives must be 20 MB or smaller.";
const TOO_MANY_ENTRIES: &str = "This archive has more than 4000 files in it.";
const TOO_BIG_INSIDE: &str = "A file inside this archive unpacks to more than 16 MB.";
const TOO_BIG_TOTAL: &str = "This archive unpacks to more than 64 MB.";
const BOMB: &str = "A file inside this archive expands far beyond its packed size.";
const MISMATCH: &str =
    "This archive's file list does not match the files in it. Zip the pack folder again.";
const ZIP64: &str =
    "Zip64 archives are not supported. Zip the pack folder again without splitting or encrypting it.";
pub const CANCELLED: &str = "cancelled";

mod write;
pub use write::pack;

struct Entry {
    path: String,
    method: u16,
    crc: u32,
    compressed: u64,
    size: u64,
    data: usize,
}

pub struct Archive<'a> {
    bytes: &'a [u8],
    entries: Vec<Entry>,
}

fn u16_at(b: &[u8], at: usize) -> Result<u16, String> {
    b.get(at..at + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or_else(|| DAMAGED.to_owned())
}

fn u32_at(b: &[u8], at: usize) -> Result<u32, String> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| DAMAGED.to_owned())
}

fn end_record(b: &[u8]) -> Option<usize> {
    let lowest = b.len().saturating_sub(65_557);
    (lowest..=b.len().checked_sub(22)?)
        .rev()
        .find(|&at| u32_at(b, at).ok() == Some(END_HEADER))
}

fn check_name(name: &str) -> Result<(), String> {
    let short: String = name.chars().take(80).collect();
    if name.is_empty() || name.len() > 255 || name.chars().any(char::is_control) {
        return Err(DAMAGED.into());
    }
    if name.contains('\\') {
        return Err(format!(
            "This archive has a file name with a backslash in it: {short}"
        ));
    }
    let drive =
        name.len() >= 2 && name.as_bytes()[0].is_ascii_alphabetic() && name.as_bytes()[1] == b':';
    if name.starts_with('/') || drive || name.split('/').any(|s| s == "..") {
        return Err(format!(
            "This archive tries to write outside its own folder: {short}"
        ));
    }
    for segment in name.split('/').filter(|s| !s.is_empty()) {
        let stem = segment.split('.').next().unwrap_or("").to_ascii_uppercase();
        let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || ((stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.len() == 4
                && stem.as_bytes()[3].is_ascii_digit()
                && stem.as_bytes()[3] != b'0');
        if reserved
            || segment.ends_with('.')
            || segment.ends_with(' ')
            || segment.contains(['<', '>', ':', '"', '|', '?', '*'])
        {
            return Err(format!(
                "This archive has a file name this computer cannot store: {short}"
            ));
        }
    }
    Ok(())
}

fn clutter(name: &str) -> bool {
    let leaf = name.rsplit('/').next().unwrap_or("");
    name.starts_with("__MACOSX/") || matches!(leaf, ".DS_Store" | "Thumbs.db")
}

fn root_of(names: &[&str]) -> Result<String, String> {
    if names.contains(&MANIFEST) {
        return Ok(String::new());
    }
    let mut roots: Vec<&str> = names
        .iter()
        .filter(|n| n.ends_with("/pack.json") && n.matches('/').count() == 1)
        .map(|n| &n[..n.len() - MANIFEST.len()])
        .collect();
    roots.dedup();
    match roots.as_slice() {
        [] => Err(NO_MANIFEST.into()),
        [root] => {
            if names.iter().any(|n| !n.starts_with(root) && !clutter(n)) {
                return Err(format!(
                    "Everything in the archive must sit inside the single {} folder.",
                    root.trim_end_matches('/')
                ));
            }
            Ok((*root).to_owned())
        }
        _ => Err("This archive holds more than one pack. Upload one pack at a time.".into()),
    }
}

impl<'a> Archive<'a> {
    pub fn open(b: &'a [u8]) -> Result<Self, String> {
        if b.len() > MAX_BYTES {
            return Err(TOO_BIG.into());
        }
        if b.get(..4) != Some(&LOCAL_HEADER.to_le_bytes()[..]) {
            return Err("That file is not a zip archive.".into());
        }
        let end = end_record(b).ok_or(DAMAGED)?;
        if end + 22 + usize::from(u16_at(b, end + 20)?) != b.len() {
            return Err(DAMAGED.into());
        }
        let count = u16_at(b, end + 10)?;
        if u16_at(b, end + 4)? != 0 || u16_at(b, end + 6)? != 0 || u16_at(b, end + 8)? != count {
            return Err(DAMAGED.into());
        }
        let size = u32_at(b, end + 12)?;
        let offset = u32_at(b, end + 16)?;
        if count == 0xFFFF || size == u32::MAX || offset == u32::MAX {
            return Err(ZIP64.into());
        }
        if usize::from(count) > MAX_ENTRIES {
            return Err(TOO_MANY_ENTRIES.into());
        }
        let (size, offset) = (size as usize, offset as usize);
        if offset + size != end {
            return Err(DAMAGED.into());
        }
        let mut raw: Vec<(String, Entry)> = Vec::new();
        let mut extents: Vec<(usize, usize)> = Vec::new();
        let mut seen = HashSet::new();
        let mut total = 0u64;
        let mut at = offset;
        while at < end {
            if raw.len() >= usize::from(count) || at + 46 > end || u32_at(b, at)? != CENTRAL_HEADER
            {
                return Err(DAMAGED.into());
            }
            let flags = u16_at(b, at + 8)?;
            let method = u16_at(b, at + 10)?;
            let crc = u32_at(b, at + 16)?;
            let compressed = u32_at(b, at + 20)?;
            let inflated = u32_at(b, at + 24)?;
            let name_len = usize::from(u16_at(b, at + 28)?);
            let extra_len = usize::from(u16_at(b, at + 30)?);
            let comment_len = usize::from(u16_at(b, at + 32)?);
            let external = u32_at(b, at + 38)?;
            let local = u32_at(b, at + 42)?;
            if compressed == u32::MAX || inflated == u32::MAX || local == u32::MAX {
                return Err(ZIP64.into());
            }
            if flags & 1 != 0 {
                return Err(
                    "This archive is encrypted. Zip the pack's files without a password.".into(),
                );
            }
            if method != 0 && method != 8 {
                return Err(DAMAGED.into());
            }
            if (external >> 16) & 0xF000 == SYMLINK_MODE {
                return Err(
                    "This archive contains a symbolic link. Zip the pack's real files instead."
                        .into(),
                );
            }
            let name_bytes = b.get(at + 46..at + 46 + name_len).ok_or(DAMAGED)?;
            if at + 46 + name_len > end {
                return Err(DAMAGED.into());
            }
            let name = std::str::from_utf8(name_bytes)
                .map_err(|_| DAMAGED.to_owned())?
                .to_owned();
            check_name(&name)?;
            if !seen.insert(name.to_lowercase()) {
                let short: String = name.chars().take(80).collect();
                return Err(format!("This archive has two files named {short}."));
            }
            let inflated = u64::from(inflated);
            if inflated > MAX_ENTRY_BYTES {
                return Err(TOO_BIG_INSIDE.into());
            }
            total += inflated;
            if total > MAX_TOTAL_BYTES {
                return Err(TOO_BIG_TOTAL.into());
            }
            let local = local as usize;
            if local + 30 > offset || u32_at(b, local)? != LOCAL_HEADER {
                return Err(DAMAGED.into());
            }
            let local_name_len = usize::from(u16_at(b, local + 26)?);
            let local_extra_len = usize::from(u16_at(b, local + 28)?);
            if b.get(local + 30..local + 30 + local_name_len) != Some(name_bytes) {
                return Err(MISMATCH.into());
            }
            let data = local + 30 + local_name_len + local_extra_len;
            let data_end = data + compressed as usize;
            if data_end > offset {
                return Err(DAMAGED.into());
            }
            extents.push((local, data_end));
            raw.push((
                name,
                Entry {
                    path: String::new(),
                    method,
                    crc,
                    compressed: u64::from(compressed),
                    size: inflated,
                    data,
                },
            ));
            at += 46 + name_len + extra_len + comment_len;
        }
        if at != end || raw.len() != usize::from(count) {
            return Err(DAMAGED.into());
        }
        if raw.is_empty() {
            return Err("That zip file is empty.".into());
        }
        extents.sort_unstable();
        if extents.windows(2).any(|w| w[0].1 > w[1].0) {
            return Err(MISMATCH.into());
        }
        let names: Vec<&str> = raw.iter().map(|(n, _)| n.as_str()).collect();
        let root = root_of(&names)?;
        let entries = raw
            .into_iter()
            .filter(|(name, _)| !clutter(name))
            .filter_map(|(name, entry)| {
                let path = name.strip_prefix(root.as_str())?.to_owned();
                (!path.is_empty()).then_some(Entry { path, ..entry })
            })
            .collect();
        Ok(Self { bytes: b, entries })
    }

    pub fn extract(&self, dest: &Path, cancel: &AtomicBool) -> Result<(), String> {
        create_dir(dest)?;
        let mut total = 0u64;
        for entry in &self.entries {
            if cancel.load(Ordering::Relaxed) {
                return Err(CANCELLED.into());
            }
            let target = inside(dest, &entry.path)?;
            if entry.path.ends_with('/') {
                create_dirs(dest, &target)?;
                continue;
            }
            if let Some(parent) = target.parent() {
                create_dirs(dest, parent)?;
            }
            let contents = self.inflate(entry, &mut total)?;
            write_new(&target, &contents)?;
        }
        Ok(())
    }

    fn inflate(&self, entry: &Entry, total: &mut u64) -> Result<Vec<u8>, String> {
        let packed = &self.bytes[entry.data..entry.data + entry.compressed as usize];
        let mut out = Vec::with_capacity(entry.size as usize);
        let mut reader: Box<dyn Read> = match entry.method {
            0 => Box::new(packed),
            _ => Box::new(flate2::read::DeflateDecoder::new(packed)),
        };
        let mut buffer = [0u8; 16 * 1024];
        loop {
            let n = reader.read(&mut buffer).map_err(|_| DAMAGED.to_owned())?;
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buffer[..n]);
            *total += n as u64;
            let written = out.len() as u64;
            if written > MAX_ENTRY_BYTES {
                return Err(TOO_BIG_INSIDE.into());
            }
            if *total > MAX_TOTAL_BYTES {
                return Err(TOO_BIG_TOTAL.into());
            }
            if written > RATIO_FLOOR && written / entry.compressed.max(1) > MAX_RATIO {
                return Err(BOMB.into());
            }
        }
        let mut crc = flate2::Crc::new();
        crc.update(&out);
        if out.len() as u64 != entry.size || crc.sum() != entry.crc {
            return Err(DAMAGED.into());
        }
        Ok(out)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checked {
    pub id: String,
    pub name: String,
    pub version: String,
}

pub fn check(bytes: &[u8], content_packs: &BTreeSet<String>) -> Result<Checked, String> {
    let archive = Archive::open(bytes)?;
    let mut total = 0u64;
    let mut manifest = None;
    for entry in archive.entries.iter().filter(|e| !e.path.ends_with('/')) {
        let contents = archive.inflate(entry, &mut total)?;
        if entry.path == MANIFEST {
            manifest = Some(contents);
        }
    }
    let manifest = manifest.ok_or(NO_MANIFEST)?;
    if manifest.len() > MANIFEST_MAX_BYTES {
        return Err("A pack.json larger than 64 KB is not a pack manifest.".into());
    }
    #[derive(serde::Deserialize)]
    struct Fields {
        id: Option<String>,
        name: Option<String>,
        version: Option<String>,
    }
    let fields: Fields = serde_json::from_slice(&manifest)
        .map_err(|_| "The pack.json in this archive is not readable JSON.".to_owned())?;
    let text = |v: Option<String>| -> String {
        let kept: String = v
            .unwrap_or_default()
            .chars()
            .filter(|c| !c.is_control())
            .collect();
        kept.trim().to_owned()
    };
    let id = fields.id.unwrap_or_default().trim().to_owned();
    let id_ok = !id.is_empty()
        && id.len() <= ID_MAX
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if !id_ok {
        return Err(format!(
            "pack.json needs an id of lowercase letters, numbers and underscores, up to {ID_MAX} characters."
        ));
    }
    if content_packs.contains(&id) {
        return Err(format!(
            "The id {id} belongs to a content pack that ships with the game. Give this pack an id of its own."
        ));
    }
    let name = text(fields.name);
    if name.is_empty() {
        return Err("pack.json needs a name.".into());
    }
    if name.encode_utf16().count() > NAME_MAX {
        return Err(format!(
            "The name in pack.json must be {NAME_MAX} characters or fewer."
        ));
    }
    let version = text(fields.version);
    if version.encode_utf16().count() > VERSION_MAX {
        return Err(format!(
            "The version in pack.json must be {VERSION_MAX} characters or fewer."
        ));
    }
    Ok(Checked { id, name, version })
}

fn inside(dest: &Path, path: &str) -> Result<PathBuf, String> {
    let mut out = dest.to_path_buf();
    for segment in path.split('/').filter(|s| !s.is_empty()) {
        let mut parts = Path::new(segment).components();
        match (parts.next(), parts.next()) {
            (Some(Component::Normal(part)), None) => out.push(part),
            _ => {
                return Err(format!(
                    "This archive tries to write outside its own folder: {path}"
                ))
            }
        }
    }
    Ok(out)
}

fn create_dir(path: &Path) -> Result<(), String> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o755);
    builder
        .create(path)
        .map_err(|e| format!("could not unpack into {}: {e}", path.display()))
}

fn create_dirs(dest: &Path, dir: &Path) -> Result<(), String> {
    let relative = dir.strip_prefix(dest).map_err(|_| DAMAGED.to_owned())?;
    let mut at = dest.to_path_buf();
    for part in relative.components() {
        at.push(part);
        match std::fs::symlink_metadata(&at) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Err(DAMAGED.into()),
            Err(_) => create_dir(&at)?,
        }
    }
    Ok(())
}

fn write_new(path: &Path, contents: &[u8]) -> Result<(), String> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o644);
    let mut file = options
        .open(path)
        .map_err(|e| format!("could not unpack {}: {e}", path.display()))?;
    file.write_all(contents)
        .map_err(|e| format!("could not unpack {}: {e}", path.display()))
}
