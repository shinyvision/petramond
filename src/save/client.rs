use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::world::RENDER_DIST;

mod graphics;
pub use graphics::{AntiAliasing, GraphicsSettings};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientSettings {
    pub render_dist: i32,
    pub fps_cap: u32,
    pub menu_fps_cap: u32,
    pub render_scale: f32,
    pub grade: bool,
    pub anti_aliasing: AntiAliasing,
    pub player_name: Option<String>,
    pub last_server: Option<String>,
    pub master_volume: f32,
    pub sound_volume: f32,
    pub music_volume: f32,
    pub particles: ParticlesMode,
    pub screen_shake: bool,
    pub bindings: petramond_input::controls::BindingSet,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParticlesMode {
    Off,
    Reduced,
    #[default]
    Full,
}

impl ParticlesMode {
    pub fn next(self) -> ParticlesMode {
        match self {
            ParticlesMode::Full => ParticlesMode::Reduced,
            ParticlesMode::Reduced => ParticlesMode::Off,
            ParticlesMode::Off => ParticlesMode::Full,
        }
    }

    pub fn density(self) -> f32 {
        match self {
            ParticlesMode::Off => 0.0,
            ParticlesMode::Reduced => 0.5,
            ParticlesMode::Full => 1.0,
        }
    }
}

impl Default for ClientSettings {
    fn default() -> Self {
        Self {
            render_dist: RENDER_DIST,
            fps_cap: 60,
            menu_fps_cap: 30,
            render_scale: 1.0,
            grade: true,
            anti_aliasing: AntiAliasing::default(),
            player_name: None,
            last_server: None,
            master_volume: 1.0,
            sound_volume: 1.0,
            music_volume: 1.0,
            particles: ParticlesMode::Full,
            screen_shake: true,
            bindings: petramond_input::controls::BindingSet::default(),
        }
    }
}

pub fn resolve_player_name(s: &ClientSettings) -> String {
    let account = crate::account::store::load().map(|saved| saved.username);
    player_name_from(s, account)
}

fn player_name_from(s: &ClientSettings, account: Option<String>) -> String {
    first_nonempty([
        std::env::var("PETRAMOND_PLAYER_NAME").ok(),
        account,
        s.player_name.clone(),
        std::env::var("USER").ok(),
        std::env::var("USERNAME").ok(),
    ])
}

fn first_nonempty(candidates: impl IntoIterator<Item = Option<String>>) -> String {
    candidates
        .into_iter()
        .flatten()
        .map(|c| c.trim().to_string())
        .find(|c| !c.is_empty())
        .unwrap_or_else(|| "Player".to_string())
}

fn path() -> PathBuf {
    super::base_data_dir().join("client.json")
}

pub fn load() -> ClientSettings {
    load_from(&path())
}

fn load_from(path: &Path) -> ClientSettings {
    let Ok(bytes) = std::fs::read(path) else {
        return ClientSettings::default();
    };
    match serde_json::from_slice(&bytes) {
        Ok(s) => s,
        Err(e) => {
            log::warn!(
                "client settings {} are unreadable ({e}); using defaults",
                path.display()
            );
            ClientSettings::default()
        }
    }
}

pub fn ensure_file() {
    if !path().exists() {
        let _ = store(&ClientSettings::default());
    }
}

pub fn store(settings: &ClientSettings) -> std::io::Result<()> {
    store_to(&path(), settings)
}

fn store_to(path: &Path, settings: &ClientSettings) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let bytes = serde_json::to_vec_pretty(settings).map_err(std::io::Error::other)?;
    petramond_persist::atomic_file::replace(path, &bytes)
}

#[cfg(test)]
mod tests;
