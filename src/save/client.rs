//! Client (per-machine) settings: `client.json` in the base data dir.
//!
//! Graphics/host knobs and the player identity that belong to the machine
//! running the game, not to a world: render distance, frame caps, render
//! scale, player name. Distinct from the per-world `settings.json`
//! (`save::settings`), which holds world state like disabled mods. An absent
//! file means defaults; unknown fields are ignored so hand-edited files
//! survive version drift. `PETRAMOND_*` env vars override the file for
//! one-off runs (see `platform::native` and [`resolve_player_name`]).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::world::RENDER_DIST;

mod graphics;
pub use graphics::{AntiAliasing, GraphicsSettings};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientSettings {
    /// Horizontal streaming radius in chunks. Fog is derived from it
    /// (`render::uniforms::fog_range`), so lowering it shrinks the drawn world
    /// instead of ending terrain before the fog fade.
    pub render_dist: i32,
    /// Gameplay frame cap (frames per second). The sim stays at 20 TPS
    /// regardless; this only bounds presentation (and GPU power draw).
    pub fps_cap: u32,
    /// Frame cap while a modal screen is up (title, pause, inventory,
    /// containers). Every frame still renders — this is a rate cut for
    /// near-static screens, not render-on-demand.
    pub menu_fps_cap: u32,
    /// Internal world-resolution scale (`0.5..=1.0`). Below 1.0 the world
    /// renders smaller and the grade pass upscales; UI stays native-res.
    /// Used with anti-aliasing Off; SSAA renders above native resolution.
    pub render_scale: f32,
    /// The colour grade. Disabling both grade and AA at native resolution
    /// skips the offscreen scene round-trip.
    pub grade: bool,
    /// Scene supersampling (Options → Graphics); UI stays unfiltered.
    pub anti_aliasing: AntiAliasing,
    /// The local player's name: multiplayer identity and the per-world save
    /// key (`players/<name>.dat`). `None` = unset; [`resolve_player_name`]
    /// falls back to the OS username.
    pub player_name: Option<String>,
    /// The address last joined via "Connect to server", as a convenience
    /// prefill for the connect screen. `None` until a first join.
    pub last_server: Option<String>,
    /// Master linear volume over every sound (`0..=1`; Options → Sound).
    pub master_volume: f32,
    /// Linear volume for non-music sound (`0..=1`).
    pub sound_volume: f32,
    /// Linear volume for the `music` sound category (`0..=1`; music itself
    /// ships later — the mixer group already exists).
    pub music_volume: f32,
    /// Decorative particle density (Options → Graphics).
    pub particles: ParticlesMode,
    /// Screen shake (Options → Graphics): the first-person camera's kicks and
    /// sway, and the hurt jitter. Off holds the view still; every animation
    /// still plays.
    pub screen_shake: bool,
    /// Remapped controls (Options → Controls). Actions absent here use their
    /// defaults, so files from before a binding existed stay valid.
    pub bindings: petramond_world::controls::BindingSet,
}

/// Decorative-particle density: emitter-derived particles (torch flames…) and
/// terrain flecks (mining dust, break bursts, splashes). Presentation-only.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParticlesMode {
    Off,
    Reduced,
    #[default]
    Full,
}

impl ParticlesMode {
    /// The Options button cycles Full → Reduced → Off → Full.
    pub fn next(self) -> ParticlesMode {
        match self {
            ParticlesMode::Full => ParticlesMode::Reduced,
            ParticlesMode::Reduced => ParticlesMode::Off,
            ParticlesMode::Off => ParticlesMode::Full,
        }
    }

    /// Spawn-count / active-count multiplier presentation applies.
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
            bindings: petramond_world::controls::BindingSet::default(),
        }
    }
}

/// The local player's effective name: `PETRAMOND_PLAYER_NAME` env >
/// client.json `player_name` > OS `$USER`/`$USERNAME` > `"Player"`.
/// Candidates are trimmed; blank ones fall through to the next.
pub fn resolve_player_name(s: &ClientSettings) -> String {
    first_nonempty([
        std::env::var("PETRAMOND_PLAYER_NAME").ok(),
        s.player_name.clone(),
        std::env::var("USER").ok(),
        std::env::var("USERNAME").ok(),
    ])
}

/// The first candidate that trims non-empty, else `"Player"`.
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

/// Read the client settings. Absent file = defaults; an unreadable file warns
/// and falls back — settings must never block launching the game.
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

/// Materialize the file with defaults on first launch, so the knobs are
/// discoverable by opening it. Never overwrites an existing file (a rewrite
/// would drop fields written by a newer build).
pub fn ensure_file() {
    if !path().exists() {
        let _ = store(&ClientSettings::default());
    }
}

/// Write the client settings (atomic tmp+rename, like every data-dir file).
pub fn store(settings: &ClientSettings) -> std::io::Result<()> {
    store_to(&path(), settings)
}

fn store_to(path: &Path, settings: &ClientSettings) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let bytes = serde_json::to_vec_pretty(settings).map_err(std::io::Error::other)?;
    super::write_atomic(path, &bytes)
}

#[cfg(test)]
mod tests;
