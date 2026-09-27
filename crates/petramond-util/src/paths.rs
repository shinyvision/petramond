use std::path::PathBuf;

pub fn base_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("PETRAMOND_DATA_DIR") {
        return PathBuf::from(dir);
    }
    directories::ProjectDirs::from("", "", "petramond")
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".petramond"))
}

pub fn installed_mods_dir() -> PathBuf {
    base_data_dir().join("mods")
}
