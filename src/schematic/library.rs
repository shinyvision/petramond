//! Atomic file operations and cheap indexing for the cross-world schematic library.
use super::{
    archive::{self, Header, Metadata, Thumbnail, HEADER_SIZE},
    Schematic,
};
use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub metadata: Metadata,
    pub header: Header,
}

pub fn directory() -> PathBuf {
    petramond_util::paths::base_data_dir().join("schematics")
}

/// Read only the fixed header and small metadata section. No cells or image decoding.
pub fn inspect(path: &Path) -> Result<Entry, String> {
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    let (header, metadata) = index(&mut file)?;
    Ok(Entry {
        path: path.into(),
        metadata,
        header,
    })
}
fn index(file: &mut fs::File) -> Result<(Header, Metadata), String> {
    let mut bytes = [0; HEADER_SIZE];
    file.read_exact(&mut bytes).map_err(|e| e.to_string())?;
    let header = Header::parse(&bytes)?;
    header.check_file_len(file.metadata().map_err(|e| e.to_string())?.len())?;
    let mut metadata = vec![0; header.metadata_len];
    file.read_exact(&mut metadata).map_err(|e| e.to_string())?;
    let metadata = header.metadata(&metadata)?;
    Ok((header, metadata))
}

pub fn list(dir: &Path) -> Result<Vec<Entry>, String> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.extension().is_none_or(|ext| ext != archive::EXTENSION) {
            continue;
        }
        match inspect(&path) {
            Ok(entry) => entries.push(entry),
            Err(error) => log::warn!("schematic {}: {error}", path.display()),
        }
    }
    sort(&mut entries);
    Ok(entries)
}
pub fn sort(entries: &mut [Entry]) {
    entries.sort_by_cached_key(|e| (e.metadata.name.to_lowercase(), e.path.clone()));
}

pub fn read(path: &Path) -> Result<Schematic, String> {
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    let (header, metadata) = index(&mut file)?;
    file.seek(SeekFrom::Start(header.payload_offset() as u64))
        .map_err(|e| e.to_string())?;
    archive::decode_payload(&header, &metadata, &mut std::io::BufReader::new(file))
}

pub fn thumbnail_bytes(entry: &Entry) -> Result<Vec<u8>, String> {
    let mut file = fs::File::open(&entry.path).map_err(|e| e.to_string())?;
    entry
        .header
        .check_file_len(file.metadata().map_err(|e| e.to_string())?.len())?;
    file.seek(SeekFrom::Start(entry.header.thumbnail_offset() as u64))
        .map_err(|e| e.to_string())?;
    let mut bytes = vec![0; entry.header.thumbnail_len];
    file.read_exact(&mut bytes).map_err(|e| e.to_string())?;
    entry.header.check_thumbnail(&bytes)?;
    Ok(bytes)
}

pub fn thumbnail(entry: &Entry) -> Result<Thumbnail, String> {
    let image = archive::decode_thumbnail(&thumbnail_bytes(entry)?)?;
    if [image.width, image.height] != entry.metadata.thumbnail_size {
        return Err("Schematic thumbnail dimensions mismatch".into());
    }
    Ok(image)
}

pub fn save(dir: &Path, schematic: &Schematic, thumbnail: &[u8]) -> Result<PathBuf, String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = dir.join(format!("{}.{}", unique_name(), archive::EXTENSION));
    save_as(&path, schematic, thumbnail)?;
    Ok(path)
}

/// Publish a complete archive without overwriting an existing file. Temporary files
/// stay outside the library index, including when encoding or disk I/O fails.
pub fn save_as(path: &Path, schematic: &Schematic, thumbnail: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("Schematic path has no directory")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    petramond_util::atomic_file::publish_new(path, |file| {
        archive::write_to(file, schematic, thumbnail).map_err(std::io::Error::other)
    })
    .map_err(|e| e.to_string())
}

fn unique_name() -> String {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{time:x}-{:x}-{:x}",
        std::process::id(),
        SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

pub fn delete(path: &Path) -> Result<(), String> {
    fs::remove_file(path).map_err(|e| e.to_string())
}
