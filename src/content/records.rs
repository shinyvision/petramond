use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{Dirs, Kind};

pub const FORMAT: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallRecord {
    pub format: u32,
    pub id: String,
    pub kind: Kind,
    pub content_id: Option<i64>,
    pub name: String,
    pub version: String,
    pub archive_sha256: String,
    pub archive_bytes: u64,
    pub pack_json_sha256: String,
    pub installed_ms: u64,
}

fn path(dirs: &Dirs, id: &str) -> PathBuf {
    dirs.content.join("installed").join(format!("{id}.json"))
}

pub fn load(dirs: &Dirs, id: &str) -> Option<InstallRecord> {
    let bytes = std::fs::read(path(dirs, id)).ok()?;
    serde_json::from_slice(&bytes)
        .ok()
        .filter(|r: &InstallRecord| r.id == id && r.format == FORMAT)
}

pub fn all(dirs: &Dirs) -> Vec<InstallRecord> {
    let Ok(entries) = std::fs::read_dir(dirs.content.join("installed")) else {
        return Vec::new();
    };
    let mut out: Vec<InstallRecord> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_str()?.strip_suffix(".json")?.to_owned();
            load(dirs, &name)
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

pub fn valid(dirs: &Dirs, record: &InstallRecord) -> bool {
    let Ok(bytes) = std::fs::read(dirs.mods.join(&record.id).join("pack.json")) else {
        return false;
    };
    pack_json_id(&bytes).as_deref() == Some(record.id.as_str())
        && super::sha256_hex(&bytes) == record.pack_json_sha256
}

pub fn write(dirs: &Dirs, record: &InstallRecord) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(record).map_err(std::io::Error::other)?;
    let path = path(dirs, &record.id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    petramond_persist::atomic_file::replace(&path, &bytes)
}

pub fn remove(dirs: &Dirs, id: &str) {
    let _ = std::fs::remove_file(path(dirs, id));
}

pub fn pack_json_id(bytes: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    value.get("id")?.as_str().map(str::to_owned)
}
