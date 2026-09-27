use std::path::{Component, Path, PathBuf};

use super::{level, settings};
use crate::net::identity::PlayerKey;
use petramond_persist::atomic_file;
use petramond_util::paths::base_data_dir;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorldInfo {
    pub name: String,
    pub dir_name: String,
    pub has_level: bool,
}

fn saves_dir() -> PathBuf {
    base_data_dir().join("saves")
}

pub fn world_dir(name: &str) -> PathBuf {
    saves_dir().join(sanitize(name))
}

pub fn dir_name_for(name: &str) -> String {
    sanitize(name)
}

fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.is_empty() {
        "world".to_string()
    } else {
        s
    }
}

pub(super) fn player_path(players_dir: &Path, key: &PlayerKey) -> PathBuf {
    players_dir.join(format!("{key}.dat"))
}

pub(super) fn legacy_player_path(players_dir: &Path, name: &str) -> PathBuf {
    players_dir.join(format!("{}.dat", sanitize(name)))
}

pub fn world_exists(name: &str) -> bool {
    world_dir(name).exists()
}

#[derive(serde::Deserialize, serde::Serialize)]
struct WorldMetadata {
    name: String,
}

pub fn write_world_metadata(name: &str) -> std::io::Result<()> {
    let dir = world_dir(name);
    std::fs::create_dir_all(&dir)?;
    let metadata = serde_json::to_vec(&WorldMetadata {
        name: name.trim().to_string(),
    })
    .map_err(std::io::Error::other)?;
    atomic_file::replace(&dir.join("world.json"), &metadata)
}

pub fn list_worlds() -> std::io::Result<Vec<WorldInfo>> {
    let dir = saves_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };

    let mut worlds = Vec::new();
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if !kind.is_dir() {
            continue;
        }
        let dir_name = entry.file_name().to_string_lossy().to_string();
        let path = entry.path();
        let name = std::fs::read(path.join("world.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<WorldMetadata>(&bytes).ok())
            .map(|m| m.name)
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| dir_name.clone());
        worlds.push(WorldInfo {
            name,
            dir_name,
            has_level: path.join("level.dat").exists(),
        });
    }
    worlds.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
            .then_with(|| a.dir_name.cmp(&b.dir_name))
    });
    Ok(worlds)
}

pub fn rename_world(dir_name: &str, new_name: &str) -> std::io::Result<()> {
    if !is_single_path_component(dir_name) {
        return Err(std::io::Error::other("invalid world directory name"));
    }
    let new_name = new_name.trim();
    if new_name.is_empty() {
        return Err(std::io::Error::other("world name cannot be empty"));
    }
    let dir = saves_dir().join(dir_name);
    if !dir.is_dir() {
        return Err(std::io::Error::other("no such world"));
    }
    let metadata = serde_json::to_vec(&WorldMetadata {
        name: new_name.to_string(),
    })
    .map_err(std::io::Error::other)?;
    atomic_file::replace(&dir.join("world.json"), &metadata)
}

pub fn delete_world(dir_name: &str) -> std::io::Result<()> {
    delete_world_at(&saves_dir(), dir_name)
}

pub fn read_world_settings(dir_name: &str) -> settings::WorldSettings {
    if !is_single_path_component(dir_name) {
        return settings::WorldSettings::default();
    }
    settings::load(&saves_dir().join(dir_name))
}

pub fn read_world_seed(dir_name: &str) -> Option<u32> {
    if !is_single_path_component(dir_name) {
        return None;
    }
    let bytes = std::fs::read(saves_dir().join(dir_name).join("level.dat")).ok()?;
    level::read_seed(&bytes)
        .inspect_err(|e| log::warn!("world '{dir_name}': {e}"))
        .ok()
}

pub fn world_size_bytes(dir_name: &str) -> u64 {
    fn walk(dir: &Path) -> u64 {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        entries
            .flatten()
            .map(|entry| match entry.file_type() {
                Ok(kind) if kind.is_dir() => walk(&entry.path()),
                Ok(kind) if kind.is_file() => entry.metadata().map(|m| m.len()).unwrap_or(0),
                _ => 0,
            })
            .sum()
    }
    if !is_single_path_component(dir_name) {
        return 0;
    }
    walk(&saves_dir().join(dir_name))
}

pub fn write_world_settings(
    dir_name: &str,
    settings: &settings::WorldSettings,
) -> std::io::Result<()> {
    if !is_single_path_component(dir_name) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "world directory must be a single path component",
        ));
    }
    settings::store(&saves_dir().join(dir_name), settings)
}

pub fn write_world_mod_baseline(
    dir_name: &str,
    disabled: &std::collections::BTreeSet<String>,
) -> std::io::Result<()> {
    if !is_single_path_component(dir_name) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "world directory must be a single path component",
        ));
    }
    let dir = saves_dir().join(dir_name);
    std::fs::create_dir_all(&dir)?;
    atomic_file::replace(
        &dir.join("mods.json"),
        &crate::modding::modset::encode_active(disabled),
    )
}

pub(super) fn delete_world_at(saves: &Path, dir_name: &str) -> std::io::Result<()> {
    if !is_single_path_component(dir_name) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "world directory must be a single path component",
        ));
    }
    match std::fs::remove_dir_all(saves.join(dir_name)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

fn is_single_path_component(name: &str) -> bool {
    let mut components = Path::new(name).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

pub fn seed_from_text(text: &str) -> u32 {
    let text = text.trim();
    if let Ok(seed) = text.parse::<u32>() {
        return seed;
    }

    let mut hash = 0x811c_9dc5u32;
    for &b in text.as_bytes() {
        hash ^= b as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

pub fn random_seed() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut z = nanos ^ ((std::process::id() as u64) << 32);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    (z ^ (z >> 31)) as u32
}
