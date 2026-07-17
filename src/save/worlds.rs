use std::path::{Component, Path, PathBuf};

use super::io::write_atomic;
use super::{level, settings};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorldInfo {
    /// User-facing world name. New worlds persist this in `world.json`; old worlds
    /// fall back to their save-directory name.
    pub name: String,
    /// Directory name under `<data>/saves/`, after path sanitization.
    pub dir_name: String,
    pub has_level: bool,
}

/// Base data dir: `~/.local/share/petramond` (Linux), `~/Library/Application
/// Support/petramond` (macOS), `%APPDATA%\petramond` (Windows). Falls back to
/// a hidden dir in the cwd if no home dir can be resolved. Also hosts the
/// user-installed mod pack root (`<data>/mods` — see `crate::assets`).
/// The user data root. `PETRAMOND_DATA_DIR` overrides it — tests point this
/// at a temp dir so saves and client-mod storage never touch the real user
/// directory.
pub(crate) fn base_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("PETRAMOND_DATA_DIR") {
        return PathBuf::from(dir);
    }
    directories::ProjectDirs::from("", "", "petramond")
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".petramond"))
}

fn saves_dir() -> PathBuf {
    base_data_dir().join("saves")
}

/// Directory for a named world: `<data>/saves/<name>/`.
pub fn world_dir(name: &str) -> PathBuf {
    saves_dir().join(sanitize(name))
}

/// The save-directory name a world NAME creates. Idempotent — a directory
/// name maps to itself, so open paths can take either at creation time. A
/// world's directory NEVER changes after creation (renames touch only the
/// `world.json` display name), so worlds must always be OPENED by
/// `WorldInfo::dir_name`, never by display name.
pub fn dir_name_for(name: &str) -> String {
    sanitize(name)
}

/// Reduce a world name to a single safe path component.
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

/// The on-disk file for a player name: `players/<sanitized name>.dat`. Names
/// sanitize through the same routine as world save directories, so any display
/// name maps to a single safe path component.
pub(super) fn player_path(players_dir: &Path, name: &str) -> PathBuf {
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
    write_atomic(&dir.join("world.json"), &metadata)
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

/// Rename a world's DISPLAY name (`world.json`); the save directory keeps its
/// original name so nothing references a moved path. Client-mod data (minimap
/// exploration, waypoints) keys on the DIRECTORY name too, so it follows a
/// renamed world by construction — moving the directory would strand it.
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
    write_atomic(&dir.join("world.json"), &metadata)
}

pub fn delete_world(dir_name: &str) -> std::io::Result<()> {
    delete_world_at(&saves_dir(), dir_name)
}

/// Read a world's per-world settings by its save-directory name (the
/// world-select / World Settings screens address worlds this way). An invalid
/// or absent directory yields defaults (all mods enabled).
pub fn read_world_settings(dir_name: &str) -> settings::WorldSettings {
    if !is_single_path_component(dir_name) {
        return settings::WorldSettings::default();
    }
    settings::load(&saves_dir().join(dir_name))
}

/// The world's seed from `level.dat`, by save-directory name — `None` for a
/// world that has never been opened (or a stale-version header). Decodes only
/// the header, so the World Settings screen shows it without opening the save.
pub fn read_world_seed(dir_name: &str) -> Option<u32> {
    if !is_single_path_component(dir_name) {
        return None;
    }
    let bytes = std::fs::read(saves_dir().join(dir_name).join("level.dat")).ok()?;
    level::read_seed(&bytes)
}

/// Total bytes of the world's save directory (recursive walk; unreadable
/// entries count 0). The World Settings screen runs this off-thread — region
/// stores can hold many files.
pub fn world_size_bytes(dir_name: &str) -> u64 {
    fn walk(dir: &Path) -> u64 {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        entries
            .flatten()
            .map(|entry| match entry.file_type() {
                Ok(kind) if kind.is_dir() => walk(&entry.path()),
                Ok(kind) if kind.is_file() => {
                    entry.metadata().map(|m| m.len()).unwrap_or(0)
                }
                _ => 0,
            })
            .sum()
    }
    if !is_single_path_component(dir_name) {
        return 0;
    }
    walk(&saves_dir().join(dir_name))
}

/// Write a world's per-world settings by its save-directory name.
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
