use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard, PoisonError};

pub(super) fn fold(name: &str) -> String {
    name.to_lowercase()
}

pub(super) fn join(root: &Path, rel: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    path.extend(rel.split('/').filter(|segment| !segment.is_empty()));
    path
}

pub(super) fn root_id(root: &Path) -> u64 {
    static IDS: LazyLock<Mutex<HashMap<PathBuf, u64>>> = LazyLock::new(Default::default);
    let mut ids = IDS.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(id) = ids.get(root) {
        return *id;
    }
    let id = ids.len() as u64;
    ids.insert(root.to_path_buf(), id);
    let swept = root.to_path_buf();
    let _ = std::thread::Builder::new()
        .name("client-mod-files-sweep".into())
        .spawn(move || sweep(&swept, std::process::id()));
    id
}

pub(super) fn sweep(dir: &Path, me: u32) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
        match left_by(&name) {
            Some(pid) if pid != me => {
                let path = entry.path();
                let removed = if is_dir {
                    std::fs::remove_dir_all(&path)
                } else {
                    std::fs::remove_file(&path)
                };
                if let Err(error) = removed {
                    log::warn!("sweep {}: {error}", path.display());
                }
            }
            Some(_) => {}
            None if is_dir && !name.starts_with('.') => sweep(&entry.path(), me),
            None => {}
        }
    }
}

fn left_by(name: &str) -> Option<u32> {
    if let Some(rest) = name.strip_prefix(".trash-") {
        return rest.split_once('-')?.0.parse().ok();
    }
    let stem = name.strip_prefix('.')?.strip_suffix(".partial")?;
    stem.rsplit('.').take(2).find_map(|segment| {
        let (pid, serial) = segment.split_once('-')?;
        serial.parse::<u64>().ok()?;
        pid.parse().ok()
    })
}

type Index = BTreeMap<PathBuf, HashMap<String, String>>;

static INDEX: LazyLock<Mutex<Index>> = LazyLock::new(Default::default);

fn index() -> MutexGuard<'static, Index> {
    INDEX.lock().unwrap_or_else(PoisonError::into_inner)
}

fn names_in(dir: &Path) -> HashMap<String, String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return HashMap::new();
    };
    entries
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| !name.starts_with('.'))
        .map(|name| (fold(&name), name))
        .collect()
}

fn case_clash(index: &mut Index, dir: &Path, name: &str) -> Option<String> {
    let folded = fold(name);
    let names = index
        .entry(dir.to_path_buf())
        .or_insert_with(|| names_in(dir));
    match names.get(&folded) {
        Some(existing) if existing != name => {}
        _ => return None,
    }
    *names = names_in(dir);
    names
        .get(&folded)
        .filter(|existing| *existing != name)
        .cloned()
}

pub(super) fn create<T>(
    root: &Path,
    rel: &str,
    create: impl FnOnce(&Path) -> std::io::Result<T>,
) -> Result<T, String> {
    let mut index = index();
    let mut dir = root.to_path_buf();
    for segment in rel.split('/') {
        if let Some(existing) = case_clash(&mut index, &dir, segment) {
            return Err(format!(
                "{rel}: '{segment}' differs only by case from '{existing}', which already exists"
            ));
        }
        dir.push(segment);
    }
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("{rel}: {error}"))?;
    }
    let made = create(&dir).map_err(|error| format!("{rel}: {error}"))?;
    let mut parent = root.to_path_buf();
    for segment in rel.split('/') {
        if let Some(names) = index.get_mut(&parent) {
            names.insert(fold(segment), segment.to_owned());
        }
        parent.push(segment);
    }
    Ok(made)
}

pub(super) fn forget(path: &Path) {
    let mut index = index();
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
        if let Some(names) = index.get_mut(parent) {
            names.remove(&fold(&name.to_string_lossy()));
        }
    }
    let under: Vec<PathBuf> = index
        .range(path.to_path_buf()..)
        .map(|(dir, _)| dir)
        .take_while(|dir| dir.starts_with(path))
        .cloned()
        .collect();
    for dir in under {
        index.remove(&dir);
    }
}
