//! Client (per-machine) settings: `client.json` in the base data dir.
//!
//! Graphics/host knobs that belong to the machine running the game, not to a
//! world: render distance, frame caps, render scale. Distinct from the
//! per-world `settings.json` (`save::settings`), which holds world state like
//! disabled mods. An absent file means defaults; unknown fields are ignored so
//! hand-edited files survive version drift. `LLAMACRAFT_*` env vars override
//! the file for one-off runs (see `platform::native`).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::world::RENDER_DIST;

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
    /// The main fill-rate lever for weak GPUs.
    pub render_scale: f32,
    /// The colour-grade post pass. Off (at scale 1.0) also skips the offscreen
    /// scene round-trip; changes the game's look — a power knob of last resort.
    pub grade: bool,
}

impl Default for ClientSettings {
    fn default() -> Self {
        Self {
            render_dist: RENDER_DIST,
            fps_cap: 60,
            menu_fps_cap: 30,
            render_scale: 1.0,
            grade: true,
        }
    }
}

fn path() -> PathBuf {
    super::base_data_dir().join("client.json")
}

/// Read the client settings. Absent file = defaults; an unreadable file warns
/// and falls back — settings must never block launching the game.
pub fn load() -> ClientSettings {
    let path = path();
    let Ok(bytes) = std::fs::read(&path) else {
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
    let path = path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let bytes = serde_json::to_vec_pretty(settings).map_err(std::io::Error::other)?;
    super::write_atomic(&path, &bytes)
}
